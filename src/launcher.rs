use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::{Duration, Instant};

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};

use crate::config::{BuiltinMode, LauncherConfig};
use crate::pty::CommandSpec;

const MAX_STDOUT_BYTES: usize = 20 * 1024 * 1024;
const MAX_STDERR_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 50_000;
const MAX_FIELD_BYTES: usize = 4_096;
const MAX_ARGUMENTS: usize = 128;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LauncherEntry {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum LauncherAction {
    OpenPane {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Execute {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    ReplacePane {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Close,
}

impl LauncherAction {
    pub fn command_spec(&self) -> Option<CommandSpec> {
        match self {
            Self::OpenPane { command, args }
            | Self::Execute { command, args }
            | Self::ReplacePane { command, args } => {
                Some(CommandSpec::new(command.clone(), args.clone()))
            }
            Self::Close => None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        let Some(command) = self.command_spec() else {
            return Ok(());
        };
        if command.program.trim().is_empty()
            || command.program.len() > MAX_FIELD_BYTES
            || command.program.contains('\0')
        {
            return Err("action command is empty or too long".to_owned());
        }
        if command.args.len() > MAX_ARGUMENTS
            || command
                .args
                .iter()
                .any(|argument| argument.len() > MAX_FIELD_BYTES || argument.contains('\0'))
        {
            return Err("action has too many arguments or an argument is too long".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum LauncherEvent {
    Listed {
        request_id: u64,
        result: Result<Vec<LauncherEntry>, String>,
    },
    Selected {
        request_id: u64,
        result: Result<LauncherAction, String>,
    },
    Executed(Result<(), String>),
}

#[derive(Clone, Debug)]
struct LauncherMode {
    name: String,
    command: Option<PathBuf>,
    builtin: Option<BuiltinMode>,
}

pub struct Launcher {
    modes: Vec<LauncherMode>,
    active_mode: usize,
    default_mode: usize,
    query: String,
    entries: Vec<LauncherEntry>,
    filtered: Vec<usize>,
    match_indices: HashMap<String, Vec<u32>>,
    selected: usize,
    open: bool,
    loading: bool,
    error: Option<String>,
    request_id: u64,
    cancellation: Option<Arc<AtomicBool>>,
    matcher: Matcher,
    width_percent: u16,
    max_results: usize,
    timeout: Duration,
}

impl Launcher {
    pub fn new(config: &LauncherConfig) -> Self {
        let modes: Vec<_> = config
            .modes
            .iter()
            .map(|mode| LauncherMode {
                name: mode.name.clone(),
                command: mode.command.clone(),
                builtin: mode.builtin,
            })
            .collect();
        let default_mode = modes
            .iter()
            .position(|mode| mode.name.eq_ignore_ascii_case(&config.default_mode))
            .unwrap_or(0);
        Self {
            modes,
            active_mode: default_mode,
            default_mode,
            query: String::new(),
            entries: Vec::new(),
            filtered: Vec::new(),
            match_indices: HashMap::new(),
            selected: 0,
            open: false,
            loading: false,
            error: None,
            request_id: 0,
            cancellation: None,
            matcher: Matcher::new(MatcherConfig::DEFAULT),
            width_percent: config.width_percent,
            max_results: config.max_results,
            timeout: Duration::from_millis(config.timeout_ms),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, events: &SyncSender<LauncherEvent>) {
        self.open = true;
        self.active_mode = self.default_mode.min(self.modes.len().saturating_sub(1));
        self.query.clear();
        self.request_list(events);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.cancel_request();
    }

    pub fn refresh(&mut self, events: &SyncSender<LauncherEvent>) {
        self.request_list(events);
    }

    pub fn next_mode(&mut self, events: &SyncSender<LauncherEvent>) {
        if self.modes.len() <= 1 {
            return;
        }
        self.active_mode = (self.active_mode + 1) % self.modes.len();
        self.query.clear();
        self.request_list(events);
    }

    pub fn previous_mode(&mut self, events: &SyncSender<LauncherEvent>) {
        if self.modes.len() <= 1 {
            return;
        }
        self.active_mode = self
            .active_mode
            .checked_sub(1)
            .unwrap_or(self.modes.len() - 1);
        self.query.clear();
        self.request_list(events);
    }

    pub fn push_query(&mut self, character: char) {
        self.query.push(character);
        self.refilter();
    }

    pub fn pop_query(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn select_previous(&mut self) {
        if self.filtered.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.filtered.len() - 1);
        }
    }

    pub fn select_next(&mut self) {
        if self.filtered.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn select(&mut self, events: &SyncSender<LauncherEvent>) {
        if !self.open || self.loading {
            return;
        }
        let Some(entry_index) = self.filtered.get(self.selected).copied() else {
            return;
        };
        let entry_id = self.entries[entry_index].id.clone();
        self.start_request(events, ScriptOperation::Select(entry_id));
    }

    pub fn handle_event(&mut self, event: LauncherEvent) -> Option<LauncherAction> {
        match event {
            LauncherEvent::Listed { request_id, result }
                if self.open && self.loading && request_id == self.request_id =>
            {
                self.loading = false;
                self.cancellation = None;
                match result {
                    Ok(entries) => {
                        self.entries = entries;
                        self.error = None;
                        self.refilter();
                    }
                    Err(error) => {
                        self.entries.clear();
                        self.filtered.clear();
                        self.error = Some(error);
                    }
                }
                None
            }
            LauncherEvent::Selected { request_id, result }
                if self.open && self.loading && request_id == self.request_id =>
            {
                self.loading = false;
                self.cancellation = None;
                match result {
                    Ok(action) => Some(action),
                    Err(error) => {
                        self.error = Some(error);
                        None
                    }
                }
            }
            LauncherEvent::Listed { .. }
            | LauncherEvent::Selected { .. }
            | LauncherEvent::Executed(_) => None,
        }
    }

    pub fn mode_name(&self) -> &str {
        self.modes
            .get(self.active_mode)
            .map_or("Launcher", |mode| mode.name.as_str())
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn match_indices(&self, entry: &LauncherEntry) -> &[u32] {
        self.match_indices.get(&entry.id).map_or(&[], Vec::as_slice)
    }

    pub fn visible_entries(&self) -> Vec<(bool, &LauncherEntry)> {
        self.visible_entries_with_limit(self.max_results)
    }

    pub fn visible_entries_with_limit(&self, limit: usize) -> Vec<(bool, &LauncherEntry)> {
        let limit = limit.min(self.max_results);
        let start = self.selected.saturating_sub(limit.saturating_sub(1));
        self.filtered
            .iter()
            .enumerate()
            .skip(start)
            .take(limit)
            .filter_map(|(index, entry_index)| {
                self.entries
                    .get(*entry_index)
                    .map(|entry| (index == self.selected, entry))
            })
            .collect()
    }

    pub fn status(&self) -> Option<&str> {
        if self.loading {
            Some("Loading…")
        } else {
            self.error.as_deref()
        }
    }

    pub fn width_percent(&self) -> u16 {
        self.width_percent
    }

    pub fn max_results(&self) -> usize {
        self.max_results
    }

    fn request_list(&mut self, events: &SyncSender<LauncherEvent>) {
        self.entries.clear();
        self.filtered.clear();
        self.match_indices.clear();
        self.selected = 0;
        self.error = None;
        self.start_request(events, ScriptOperation::List);
    }

    fn start_request(&mut self, events: &SyncSender<LauncherEvent>, operation: ScriptOperation) {
        self.cancel_request();
        let Some(mode) = self.modes.get(self.active_mode).cloned() else {
            self.error = Some("no launcher modes are configured".to_owned());
            return;
        };
        self.request_id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        self.loading = true;
        let request_id = self.request_id;
        let cancellation = Arc::new(AtomicBool::new(false));
        self.cancellation = Some(Arc::clone(&cancellation));
        let timeout = self.timeout;
        let events = events.clone();
        thread::spawn(move || {
            let result = run_script(&mode, &operation, timeout, &cancellation);
            let event = match operation {
                ScriptOperation::List => LauncherEvent::Listed {
                    request_id,
                    result: result.and_then(|output| parse_entries(&output)),
                },
                ScriptOperation::Select(_) => LauncherEvent::Selected {
                    request_id,
                    result: result.and_then(|output| parse_action(&output)),
                },
            };
            let _ = events.send(event);
        });
    }

    fn cancel_request(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.store(true, Ordering::Relaxed);
        }
        self.loading = false;
        self.request_id = 0;
    }

    fn refilter(&mut self) {
        self.match_indices.clear();
        let pattern = Pattern::new(
            &self.query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut buffer = Vec::new();
        let mut scored: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let mut indices = Vec::new();
                let score = pattern.indices(
                    Utf32Str::new(&entry.label, &mut buffer),
                    &mut self.matcher,
                    &mut indices,
                )?;
                indices.sort_unstable();
                indices.dedup();
                if !indices.is_empty() {
                    self.match_indices.insert(entry.id.clone(), indices);
                }
                Some((index, score))
            })
            .collect();
        scored.sort_by(|(left_index, left_score), (right_index, right_score)| {
            right_score.cmp(left_score).then_with(|| {
                self.entries[*left_index]
                    .label
                    .cmp(&self.entries[*right_index].label)
            })
        });
        self.filtered = scored.into_iter().map(|(index, _)| index).collect();
        self.selected = 0;
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        self.cancel_request();
    }
}

enum ScriptOperation {
    List,
    Select(String),
}

pub fn execute_async(command: CommandSpec, events: SyncSender<LauncherEvent>) {
    thread::spawn(move || {
        let result = Command::new(&command.program)
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("failed to execute {}: {error}", command.program))
            .and_then(|status| {
                status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("{} exited with status {status}", command.program))
            });
        let _ = events.send(LauncherEvent::Executed(result));
    });
}

fn run_script(
    mode: &LauncherMode,
    operation: &ScriptOperation,
    timeout: Duration,
    cancellation: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut command = match mode.builtin {
        Some(BuiltinMode::Ssh) => {
            let mut command =
                Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
            command.arg("--ssh-picker");
            command
        }
        None => Command::new(mode.command.as_ref().ok_or("missing script command")?),
    };
    match operation {
        ScriptOperation::List => {
            command.arg("list");
        }
        ScriptOperation::Select(entry_id) => {
            command.args(["select", entry_id]);
        }
    }
    let child = command
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run mode {}: {error}", mode.name))?;
    let mut child = ScriptChild {
        child,
        group_terminated: false,
    };
    let mut stdout_pipe = child.child.stdout.take().ok_or("missing script stdout")?;
    let mut stderr_pipe = child.child.stderr.take().ok_or("missing script stderr")?;
    set_nonblocking(&stdout_pipe)?;
    set_nonblocking(&stderr_pipe)?;
    let mut stdout = BoundedOutput::default();
    let mut stderr = BoundedOutput::default();
    let started = Instant::now();

    loop {
        if cancellation.load(Ordering::Relaxed) {
            return Err("launcher request cancelled".to_owned());
        }
        if started.elapsed() >= timeout {
            return Err(format!("mode {} timed out", mode.name));
        }
        for _ in 0..32 {
            let previous_length = stdout.bytes.len() + stderr.bytes.len();
            stdout.read_available(&mut stdout_pipe, MAX_STDOUT_BYTES)?;
            stderr.read_available(&mut stderr_pipe, MAX_STDERR_BYTES)?;
            if stdout.exceeded
                || stderr.exceeded
                || stdout.bytes.len() + stderr.bytes.len() == previous_length
            {
                break;
            }
        }
        if stdout.exceeded || stderr.exceeded {
            return Err(format!("mode {} exceeded the output limit", mode.name));
        }
        if stdout.eof && stderr.eof && child.exited()? {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill_group();
    let status = child.child.wait().map_err(|error| error.to_string())?;
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr.bytes);
        let detail = stderr.trim();
        return Err(if detail.is_empty() {
            format!("mode {} exited with status {status}", mode.name)
        } else {
            format!("mode {} failed: {detail}", mode.name)
        });
    }
    Ok(stdout.bytes)
}

#[derive(Default)]
struct BoundedOutput {
    bytes: Vec<u8>,
    exceeded: bool,
    eof: bool,
}

impl BoundedOutput {
    fn read_available(&mut self, reader: &mut impl Read, limit: usize) -> Result<(), String> {
        if self.eof {
            return Ok(());
        }
        let mut buffer = [0_u8; 8192];
        match reader.read(&mut buffer) {
            Ok(0) => self.eof = true,
            Ok(length) => {
                let remaining = limit.saturating_sub(self.bytes.len());
                self.bytes
                    .extend_from_slice(&buffer[..length.min(remaining)]);
                self.exceeded |= length > remaining;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(format!("failed to read script output: {error}")),
        }
        Ok(())
    }
}

fn set_nonblocking(pipe: &impl AsRawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

struct ScriptChild {
    child: std::process::Child,
    group_terminated: bool,
}

impl ScriptChild {
    fn exited(&self) -> Result<bool, String> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(unsafe { info.si_pid() } != 0)
    }

    fn kill_group(&mut self) {
        if !self.group_terminated {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            self.group_terminated = true;
        }
    }
}

impl Drop for ScriptChild {
    fn drop(&mut self) {
        self.kill_group();
        let _ = self.child.wait();
    }
}

pub fn parse_entries(output: &[u8]) -> Result<Vec<LauncherEntry>, String> {
    let text = std::str::from_utf8(output).map_err(|_| "list output is not UTF-8".to_owned())?;
    let mut entries = Vec::new();
    let mut ids = HashSet::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if entries.len() >= MAX_ENTRIES {
            return Err("list output contains too many entries".to_owned());
        }
        let entry: LauncherEntry = serde_json::from_str(line)
            .map_err(|error| format!("invalid JSON on line {}: {error}", line_number + 1))?;
        if entry.id.trim().is_empty()
            || entry.label.trim().is_empty()
            || entry.id.contains('\0')
            || entry.label.contains('\0')
            || entry.id.len() > MAX_FIELD_BYTES
            || entry.label.len() > MAX_FIELD_BYTES
            || entry
                .description
                .as_ref()
                .is_some_and(|description| description.len() > MAX_FIELD_BYTES)
        {
            return Err(format!("invalid entry on line {}", line_number + 1));
        }
        if !ids.insert(entry.id.clone()) {
            return Err(format!("duplicate entry id {}", entry.id));
        }
        entries.push(entry);
    }
    Ok(entries)
}

pub fn parse_action(output: &[u8]) -> Result<LauncherAction, String> {
    let action: LauncherAction =
        serde_json::from_slice(output).map_err(|error| format!("invalid action JSON: {error}"))?;
    action.validate()?;
    Ok(action)
}
