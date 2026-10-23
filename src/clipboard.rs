use std::fs::OpenOptions;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use arboard::Clipboard as NativeClipboard;
use crossterm::ExecutableCommand;
use crossterm::clipboard::CopyToClipboard;

const OSC52_MAX_BYTES: usize = 100_000;

pub struct Clipboard {
    native: Option<NativeClipboard>,
    environment: ClipboardEnvironment,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClipboardEnvironment {
    ssh: bool,
    tmux: bool,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            native: None,
            environment: ClipboardEnvironment::detect(),
        }
    }

    pub fn copy(&mut self, text: &str) -> Result<()> {
        validate_nonempty(text)?;
        if !self.environment.ssh && self.copy_natively(text).is_ok() {
            return Ok(());
        }
        copy_through_terminal(text, self.environment.tmux)
    }

    fn copy_natively(&mut self, text: &str) -> Result<()> {
        if self.native.is_none() {
            self.native =
                Some(NativeClipboard::new().context("native X11 clipboard is unavailable")?);
        }
        self.native
            .as_mut()
            .context("native X11 clipboard was not initialized")?
            .set_text(text)
            .context("failed to set native X11 clipboard text")
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardEnvironment {
    fn detect() -> Self {
        Self {
            ssh: ["SSH_TTY", "SSH_CONNECTION", "SSH_CLIENT"]
                .into_iter()
                .any(|name| std::env::var_os(name).is_some()),
            tmux: ["TMUX", "TMUX_PANE"]
                .into_iter()
                .any(|name| std::env::var_os(name).is_some()),
        }
    }
}

fn validate_nonempty(text: &str) -> Result<()> {
    if text.is_empty() {
        bail!("refusing to copy an empty selection");
    }
    Ok(())
}

fn copy_through_terminal(text: &str, tmux: bool) -> Result<()> {
    if tmux && copy_through_tmux(text).is_ok() {
        return Ok(());
    }
    validate_terminal_payload(text)?;
    #[cfg(unix)]
    if let Ok(tty) = OpenOptions::new().write(true).open("/dev/tty")
        && write_terminal_copy(tty, text).is_ok()
    {
        return Ok(());
    }
    write_terminal_copy(io::stdout().lock(), text)
}

fn copy_through_tmux(text: &str) -> Result<()> {
    let mut child = Command::new("tmux")
        .args(["load-buffer", "-w", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to start tmux clipboard command")?;
    let mut stdin = child
        .stdin
        .take()
        .context("failed to open tmux clipboard input")?;
    stdin
        .write_all(text.as_bytes())
        .context("failed to write tmux clipboard input")?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .context("failed to wait for tmux clipboard command")?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if stderr.is_empty() {
            anyhow!("tmux clipboard command exited with {}", output.status)
        } else {
            anyhow!("tmux clipboard command failed: {stderr}")
        })
    }
}

fn write_terminal_copy(mut writer: impl Write, text: &str) -> Result<()> {
    writer
        .execute(CopyToClipboard::to_clipboard_from(text))
        .context("failed to write OSC 52 clipboard command")?;
    writer
        .flush()
        .context("failed to flush OSC 52 clipboard command")
}

fn validate_terminal_payload(text: &str) -> Result<()> {
    validate_nonempty(text)?;
    if text.len() > OSC52_MAX_BYTES {
        bail!(
            "clipboard selection is too large ({} bytes; maximum is {OSC52_MAX_BYTES})",
            text.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        OSC52_MAX_BYTES, validate_nonempty, validate_terminal_payload, write_terminal_copy,
    };

    #[test]
    fn empty_clipboard_payloads_are_rejected() {
        assert!(validate_nonempty("").is_err());
        assert!(validate_terminal_payload("").is_err());
    }

    #[test]
    fn oversized_osc52_payloads_are_rejected() {
        assert!(validate_terminal_payload(&"x".repeat(OSC52_MAX_BYTES + 1)).is_err());
    }

    #[test]
    fn crossterm_emits_multiline_utf8_clipboard_commands_repeatedly() {
        let mut output = Vec::new();
        write_terminal_copy(&mut output, "first α中🙂\nsecond café").unwrap();
        let once = output.clone();
        write_terminal_copy(&mut output, "first α中🙂\nsecond café").unwrap();
        assert!(once.starts_with(b"\x1b]52;c;"));
        assert!(once.ends_with(b"\x1b\\"));
        assert_eq!(output, [once.as_slice(), once.as_slice()].concat());
    }
}
