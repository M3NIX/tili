use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::{Handle as SignalHandle, Signals};
use tastty_core::{Position, TerminalMode};

use crate::actions::{Action, ResizeModeAction, action_for_key, resize_action_for_key};
use crate::config::Config;
use crate::input::{encode_key, encode_paste};
use crate::launcher::{Launcher, LauncherAction, LauncherEvent, execute_async};
use crate::pane::Pane;
use crate::pty::{CommandSpec, PtyEvent};
use crate::renderer::{RenderOverlays, TerminalRenderer, WorkspaceBar};
use crate::tiling::{MinimumSize, Orientation, PaneGeometry, PaneId, Rect, TilingTree};
use crate::workspace::Workspaces;

const MINIMUM_PANE_SIZE: MinimumSize = MinimumSize::new(3, 3);

enum AppEvent {
    Terminal(TerminalEvent),
    TerminalError(String),
    Pty(PtyEvent),
    Launcher(LauncherEvent),
    Shutdown,
}

struct App {
    panes: HashMap<PaneId, Pane>,
    retain_failed_panes: HashSet<PaneId>,
    workspaces: Workspaces,
    selected_split: Orientation,
    resize_step: u16,
    input_mode: InputMode,
    pending_replace: Option<CommandSpec>,
    next_pane_id: u64,
    terminal_bounds: Rect,
    shell: CommandSpec,
    cwd: PathBuf,
    pty_tx: SyncSender<PtyEvent>,
    launcher: Launcher,
    launcher_tx: SyncSender<LauncherEvent>,
    notice: Option<String>,
    selection: Option<PaneId>,
    pending_clipboard: Option<String>,
}

impl App {
    fn new(
        columns: u16,
        rows: u16,
        pty_tx: SyncSender<PtyEvent>,
        launcher_tx: SyncSender<LauncherEvent>,
        config: &Config,
    ) -> Result<Self> {
        let initial = PaneId(1);
        let shell = CommandSpec::new(config.general.shell.clone(), std::iter::empty::<String>());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let terminal_bounds = Rect::new(0, 0, columns, rows);
        let content = content_rect(pane_area(terminal_bounds));
        let pane = Pane::spawn(
            initial,
            &shell,
            Some(&cwd),
            content.width,
            content.height,
            pty_tx.clone(),
        )?;
        let mut panes = HashMap::new();
        panes.insert(initial, pane);
        Ok(Self {
            panes,
            retain_failed_panes: HashSet::new(),
            workspaces: Workspaces::new(initial),
            selected_split: config.general.default_split.into(),
            resize_step: config.general.resize_step,
            input_mode: InputMode::Normal,
            pending_replace: None,
            next_pane_id: 2,
            terminal_bounds,
            shell,
            cwd,
            pty_tx,
            launcher: Launcher::new(&config.launcher),
            launcher_tx,
            notice: None,
            selection: None,
            pending_clipboard: None,
        })
    }

    fn handle_terminal(&mut self, event: TerminalEvent) -> Result<EventResult> {
        match event {
            TerminalEvent::Key(key) => {
                self.notice = None;
                if is_host_copy_shortcut(key) {
                    let _ = self.queue_selected_text();
                    return Ok(if self.pending_clipboard.is_some() {
                        EventResult::Redraw
                    } else {
                        EventResult::Continue
                    });
                }
                if is_host_paste_shortcut(key) {
                    return Ok(EventResult::Continue);
                }
                self.clear_selection();
                if self.input_mode == InputMode::Resize {
                    return self.handle_resize_key(key);
                }
                if self.input_mode == InputMode::ConfirmReplace {
                    return self.handle_replace_confirmation(key);
                }
                if self.launcher.is_open() {
                    return Ok(self.handle_launcher_key(key));
                }
                if let Some(action) = action_for_key(key) {
                    return self.dispatch(action);
                }
                let Some(focused) = self.layout().focused() else {
                    return Ok(EventResult::Exit);
                };
                let Some(pane) = self.panes.get_mut(&focused) else {
                    return Ok(EventResult::Exit);
                };
                if let Some(bytes) =
                    encode_key(key, pane.screen().mode(TerminalMode::ApplicationCursor))
                {
                    pane.write_input(&bytes)?;
                }
                Ok(EventResult::Continue)
            }
            TerminalEvent::Paste(text) => {
                self.clear_selection();
                if self.launcher.is_open() {
                    for character in text.chars().filter(|character| !character.is_control()) {
                        self.launcher.push_query(character);
                    }
                    return Ok(EventResult::Redraw);
                }
                let Some(focused) = self.layout().focused() else {
                    return Ok(EventResult::Exit);
                };
                let Some(pane) = self.panes.get_mut(&focused) else {
                    return Ok(EventResult::Exit);
                };
                let bytes = encode_paste(&text, pane.screen().mode(TerminalMode::BracketedPaste));
                pane.write_input(&bytes)?;
                Ok(EventResult::Continue)
            }
            TerminalEvent::Resize(columns, rows) => {
                self.clear_selection();
                self.terminal_bounds = Rect::new(0, 0, columns, rows);
                self.resize_visible_panes()?;
                Ok(EventResult::Redraw)
            }
            TerminalEvent::Mouse(mouse) => self.handle_mouse(mouse),
            _ => Ok(EventResult::Continue),
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<EventResult> {
        if self.launcher.is_open() || self.input_mode != InputMode::Normal {
            return Ok(EventResult::Continue);
        }
        let visible = visible_layout(
            self.layout(),
            self.fullscreen(),
            pane_area(self.terminal_bounds),
            MINIMUM_PANE_SIZE,
        );

        if mouse.kind == MouseEventKind::Moved {
            let mut redraw = false;
            if let Some(hovered) = pane_at(&visible, mouse.column, mouse.row)
                && Some(hovered) != self.layout().focused()
            {
                self.layout_mut().focus(hovered)?;
                redraw = true;
            }
            let forwarded = self.forward_mouse_to_pane(&visible, mouse)?;
            return Ok(if redraw || forwarded {
                EventResult::Redraw
            } else {
                EventResult::Continue
            });
        }

        let selected_pane = self.selection;
        if matches!(
            mouse.kind,
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
        ) && let Some(pane_id) = selected_pane
            && let Some(position) = pane_position(&visible, pane_id, mouse.column, mouse.row, true)
        {
            let _ = self
                .panes
                .get_mut(&pane_id)
                .is_some_and(|pane| pane.selection_extend(position));
            if mouse.kind == MouseEventKind::Up(MouseButton::Left) && !self.queue_selected_text() {
                self.clear_selection();
            }
            return Ok(EventResult::Redraw);
        }

        let Some((pane_id, position)) = pane_content_at(&visible, mouse.column, mouse.row) else {
            return Ok(EventResult::Continue);
        };
        let reports_mouse = self.panes.get(&pane_id).is_some_and(Pane::reports_mouse);
        let select_locally = mouse.modifiers.contains(KeyModifiers::SHIFT) || !reports_mouse;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if select_locally => {
                self.clear_selection();
                if self
                    .panes
                    .get_mut(&pane_id)
                    .is_some_and(|pane| pane.selection_start(position))
                {
                    self.selection = Some(pane_id);
                }
                Ok(EventResult::Redraw)
            }
            MouseEventKind::ScrollUp if !reports_mouse => {
                let changed = self
                    .panes
                    .get_mut(&pane_id)
                    .is_some_and(|pane| pane.scroll_up(3));
                Ok(if changed {
                    EventResult::Redraw
                } else {
                    EventResult::Continue
                })
            }
            MouseEventKind::ScrollDown if !reports_mouse => {
                let changed = self
                    .panes
                    .get_mut(&pane_id)
                    .is_some_and(|pane| pane.scroll_down(3));
                Ok(if changed {
                    EventResult::Redraw
                } else {
                    EventResult::Continue
                })
            }
            _ => {
                self.clear_selection();
                let forwarded = self.forward_mouse(pane_id, position, mouse)?;
                Ok(if forwarded {
                    EventResult::Redraw
                } else {
                    EventResult::Continue
                })
            }
        }
    }

    fn forward_mouse_to_pane(
        &mut self,
        layout: &[PaneGeometry],
        mouse: MouseEvent,
    ) -> Result<bool> {
        let Some((pane_id, position)) = pane_content_at(layout, mouse.column, mouse.row) else {
            return Ok(false);
        };
        self.forward_mouse(pane_id, position, mouse)
    }

    fn forward_mouse(
        &mut self,
        pane_id: PaneId,
        position: Position,
        mouse: MouseEvent,
    ) -> Result<bool> {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return Ok(false);
        };
        pane.write_mouse(MouseEvent {
            kind: mouse.kind,
            column: position.col,
            row: position.row,
            modifiers: mouse.modifiers,
        })
    }

    fn queue_selected_text(&mut self) -> bool {
        self.pending_clipboard = None;
        let Some(pane_id) = self.selection else {
            return false;
        };
        let Some(pane) = self.panes.get(&pane_id) else {
            return false;
        };
        let Some(text) = pane.selected_text() else {
            return false;
        };
        self.pending_clipboard = Some(text);
        true
    }

    fn clear_selection(&mut self) {
        if let Some(pane_id) = self.selection.take()
            && let Some(pane) = self.panes.get_mut(&pane_id)
        {
            pane.selection_clear();
        }
    }

    fn handle_pty(&mut self, event: PtyEvent) -> Result<EventResult> {
        match event {
            PtyEvent::Output(pane_id, bytes) => {
                if let Some(pane) = self.panes.get_mut(&pane_id) {
                    pane.process_output(&bytes)?;
                    Ok(if self.layout().contains(pane_id) {
                        EventResult::Redraw
                    } else {
                        EventResult::Continue
                    })
                } else {
                    Ok(EventResult::Continue)
                }
            }
            PtyEvent::Eof(pane_id) => self.handle_pane_exit(pane_id, None),
            PtyEvent::ReadError(pane_id, error) => {
                if !self.panes.contains_key(&pane_id) {
                    return Ok(EventResult::Continue);
                }
                self.handle_pane_exit(pane_id, Some(format!("PTY read failed: {error}")))
            }
        }
    }

    fn handle_launcher_event(&mut self, event: LauncherEvent) -> Result<EventResult> {
        if let LauncherEvent::Executed(result) = event {
            self.notice = Some(match result {
                Ok(()) => "Command completed".to_owned(),
                Err(error) => error,
            });
            return Ok(EventResult::Redraw);
        }
        let Some(action) = self.launcher.handle_event(event) else {
            return Ok(EventResult::Redraw);
        };
        self.launcher.close();
        if let Err(error) = self.execute_launcher_action(action) {
            self.notice = Some(format!("Launcher action failed: {error:#}"));
        }
        Ok(EventResult::Redraw)
    }

    fn execute_launcher_action(&mut self, action: LauncherAction) -> Result<()> {
        match action {
            LauncherAction::OpenPane { command, args } => {
                let pane = self.open_pane(CommandSpec::new(command, args), self.cwd.clone())?;
                self.retain_failed_panes.insert(pane);
            }
            LauncherAction::Execute { command, args } => {
                execute_async(CommandSpec::new(command, args), self.launcher_tx.clone());
                self.notice = Some("Command started".to_owned());
            }
            LauncherAction::ReplacePane { command, args } => {
                self.pending_replace = Some(CommandSpec::new(command, args));
                self.input_mode = InputMode::ConfirmReplace;
            }
            LauncherAction::Close => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, action: Action) -> Result<EventResult> {
        match action {
            Action::OpenPane => {
                if let Err(error) = self.open_pane(self.shell.clone(), self.cwd.clone()) {
                    self.notice = Some(format!("Could not open pane: {error:#}"));
                }
                Ok(EventResult::Redraw)
            }
            Action::OpenPaneInWorkingDirectory => {
                let cwd = self
                    .layout()
                    .focused()
                    .and_then(|focused| self.panes.get(&focused))
                    .and_then(Pane::current_working_directory)
                    .unwrap_or_else(|| self.cwd.clone());
                if let Err(error) = self.open_pane(self.shell.clone(), cwd) {
                    self.notice = Some(format!("Could not open pane: {error:#}"));
                }
                Ok(EventResult::Redraw)
            }
            Action::ClosePane => {
                let Some(focused) = self.layout().focused() else {
                    return Ok(EventResult::Exit);
                };
                self.close_pane(focused)
            }
            Action::SetSplit(orientation) => {
                self.selected_split = orientation;
                Ok(EventResult::Redraw)
            }
            Action::ToggleSplit => {
                self.selected_split = match self.selected_split {
                    Orientation::Horizontal => Orientation::Vertical,
                    Orientation::Vertical => Orientation::Horizontal,
                };
                Ok(EventResult::Redraw)
            }
            Action::Focus(direction) => {
                if self.fullscreen().is_some() {
                    return Ok(EventResult::Continue);
                }
                let terminal_bounds = pane_area(self.terminal_bounds);
                let changed = self
                    .layout_mut()
                    .focus_direction(direction, terminal_bounds, MINIMUM_PANE_SIZE)
                    .is_some();
                Ok(if changed {
                    EventResult::Redraw
                } else {
                    EventResult::Continue
                })
            }
            Action::Move(direction) => {
                if self.fullscreen().is_some() {
                    return Ok(EventResult::Continue);
                }
                let terminal_bounds = pane_area(self.terminal_bounds);
                let changed =
                    self.layout_mut()
                        .move_focused(direction, terminal_bounds, MINIMUM_PANE_SIZE);
                if changed {
                    self.resize_visible_panes()?;
                    Ok(EventResult::Redraw)
                } else {
                    Ok(EventResult::Continue)
                }
            }
            Action::EnterResizeMode => {
                self.input_mode = InputMode::Resize;
                Ok(EventResult::Redraw)
            }
            Action::ToggleFullscreen => {
                self.workspaces.active_mut().fullscreen = if self.fullscreen().is_some() {
                    None
                } else {
                    self.layout().focused()
                };
                self.resize_visible_panes()?;
                Ok(EventResult::Redraw)
            }
            Action::OpenLauncher => {
                self.launcher.open(&self.launcher_tx);
                Ok(EventResult::Redraw)
            }
            Action::SwitchWorkspace(workspace) => self.switch_workspace(workspace),
            Action::MoveToWorkspace(workspace) => self.move_to_workspace(workspace),
            Action::ReloadConfig => Ok(EventResult::ReloadConfig),
        }
    }

    fn handle_resize_key(&mut self, key: crossterm::event::KeyEvent) -> Result<EventResult> {
        match resize_action_for_key(key) {
            Some(ResizeModeAction::Exit) => {
                self.input_mode = InputMode::Normal;
                Ok(EventResult::Redraw)
            }
            Some(ResizeModeAction::Resize(resize)) if self.fullscreen().is_none() => {
                let terminal_bounds = pane_area(self.terminal_bounds);
                let resize_step = self.resize_step;
                let changed = self.layout_mut().resize_focused(
                    resize,
                    resize_step,
                    terminal_bounds,
                    MINIMUM_PANE_SIZE,
                );
                if changed {
                    self.resize_visible_panes()?;
                    Ok(EventResult::Redraw)
                } else {
                    Ok(EventResult::Continue)
                }
            }
            Some(ResizeModeAction::Resize(_)) | None => Ok(EventResult::Continue),
        }
    }

    fn handle_replace_confirmation(&mut self, key: KeyEvent) -> Result<EventResult> {
        if matches!(key.kind, KeyEventKind::Release) {
            return Ok(EventResult::Continue);
        }
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                self.input_mode = InputMode::Normal;
                if let Some(command) = self.pending_replace.take()
                    && let Err(error) = self.replace_focused(command)
                {
                    self.notice = Some(format!("Could not replace pane: {error:#}"));
                }
                Ok(EventResult::Redraw)
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.pending_replace = None;
                self.input_mode = InputMode::Normal;
                Ok(EventResult::Redraw)
            }
            _ => Ok(EventResult::Continue),
        }
    }

    fn handle_launcher_key(&mut self, key: KeyEvent) -> EventResult {
        if matches!(key.kind, KeyEventKind::Release) {
            return EventResult::Continue;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => self.launcher.close(),
            KeyCode::Left if !control => self.launcher.previous_mode(&self.launcher_tx),
            KeyCode::Right if !control => self.launcher.next_mode(&self.launcher_tx),
            KeyCode::Up => self.launcher.select_previous(),
            KeyCode::Down => self.launcher.select_next(),
            KeyCode::Enter => self.launcher.select(&self.launcher_tx),
            KeyCode::Backspace if !control => self.launcher.pop_query(),
            KeyCode::Tab if control && shift => self.launcher.previous_mode(&self.launcher_tx),
            KeyCode::BackTab if control => self.launcher.previous_mode(&self.launcher_tx),
            KeyCode::Tab if control => self.launcher.next_mode(&self.launcher_tx),
            KeyCode::Char('r' | 'R') if control => self.launcher.refresh(&self.launcher_tx),
            KeyCode::Char(character) if !control && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.launcher.push_query(character);
            }
            _ => return EventResult::Continue,
        }
        EventResult::Redraw
    }

    fn open_pane(&mut self, command: CommandSpec, cwd: PathBuf) -> Result<PaneId> {
        let pane_id = PaneId(self.next_pane_id);
        let mut next_layout = self.layout().clone();
        next_layout
            .split_focused(pane_id, self.selected_split)
            .context("failed to split focused pane")?;
        let geometry = visible_layout(
            &next_layout,
            None,
            pane_area(self.terminal_bounds),
            MINIMUM_PANE_SIZE,
        )
        .into_iter()
        .find(|geometry| geometry.pane == pane_id)
        .context("new pane has no layout geometry")?;
        let content = content_rect(geometry.rect);
        let pane = Pane::spawn(
            pane_id,
            &command,
            Some(&cwd),
            content.width,
            content.height,
            self.pty_tx.clone(),
        )?;

        self.next_pane_id = self.next_pane_id.saturating_add(1);
        self.workspaces.active_mut().fullscreen = None;
        self.workspaces.active_mut().layout = next_layout;
        self.panes.insert(pane_id, pane);
        self.resize_visible_panes()?;
        Ok(pane_id)
    }

    fn replace_focused(&mut self, command: CommandSpec) -> Result<()> {
        let old_pane_id = self
            .layout()
            .focused()
            .context("cannot replace a pane in an empty layout")?;
        let cwd = self
            .panes
            .get(&old_pane_id)
            .and_then(Pane::current_working_directory)
            .unwrap_or_else(|| self.cwd.clone());
        let new_pane_id = PaneId(self.next_pane_id);
        let geometry = visible_layout(
            self.layout(),
            self.fullscreen(),
            pane_area(self.terminal_bounds),
            MINIMUM_PANE_SIZE,
        )
        .into_iter()
        .find(|geometry| geometry.pane == old_pane_id)
        .context("focused pane has no layout geometry")?;
        let content = content_rect(geometry.rect);

        let pane = Pane::spawn(
            new_pane_id,
            &command,
            Some(&cwd),
            content.width,
            content.height,
            self.pty_tx.clone(),
        )?;

        self.layout_mut().replace_pane(old_pane_id, new_pane_id)?;
        self.panes.remove(&old_pane_id);
        self.retain_failed_panes.remove(&old_pane_id);
        if self.fullscreen() == Some(old_pane_id) {
            self.workspaces.active_mut().fullscreen = Some(new_pane_id);
        }
        self.panes.insert(new_pane_id, pane);
        self.retain_failed_panes.insert(new_pane_id);
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        self.resize_visible_panes()
    }

    fn handle_pane_exit(
        &mut self,
        pane_id: PaneId,
        fallback: Option<String>,
    ) -> Result<EventResult> {
        if !self.panes.contains_key(&pane_id) {
            return Ok(EventResult::Continue);
        }
        let status = {
            let pane = self
                .panes
                .get_mut(&pane_id)
                .context("exited pane disappeared")?;
            let mut status = pane.try_wait()?;
            // PTY EOF and the child's wait status are reported by separate
            // kernel paths. Give waitpid a short chance to observe an exit
            // before recording the generic fallback below.
            for _ in 0..10 {
                if status.is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
                status = pane.try_wait()?;
            }
            status
        };
        if !self.retain_failed_panes.contains(&pane_id)
            || status.as_ref().is_some_and(|status| status.success())
        {
            return self.close_pane(pane_id);
        }

        let description = status
            .map(|status| status.to_string())
            .or(fallback)
            .unwrap_or_else(|| "PTY closed before an exit status was available".to_owned());
        self.panes
            .get_mut(&pane_id)
            .context("exited pane disappeared")?
            .mark_exited(&description);
        Ok(EventResult::Redraw)
    }

    fn close_pane(&mut self, pane_id: PaneId) -> Result<EventResult> {
        let workspace_index = self
            .workspaces
            .workspace_containing(pane_id)
            .context("pane is not present in any workspace")?;
        let active = workspace_index == self.workspaces.active_index();
        let workspace = self
            .workspaces
            .get_mut(workspace_index)
            .context("workspace index became invalid")?;
        if workspace.fullscreen == Some(pane_id) {
            workspace.fullscreen = None;
        }
        workspace.layout.close(pane_id)?;
        if self.selection == Some(pane_id) {
            self.selection = None;
        }
        self.panes.remove(&pane_id);
        self.retain_failed_panes.remove(&pane_id);
        if self.panes.is_empty() {
            return Ok(EventResult::Exit);
        }
        if active {
            if self.layout().is_empty() {
                self.workspaces
                    .switch_to_last_open()
                    .context("no non-empty workspace remains")?;
            }
            self.resize_visible_panes()?;
        }
        Ok(EventResult::Redraw)
    }

    fn resize_visible_panes(&mut self) -> Result<()> {
        for geometry in visible_layout(
            self.layout(),
            self.fullscreen(),
            pane_area(self.terminal_bounds),
            MINIMUM_PANE_SIZE,
        ) {
            if let Some(pane) = self.panes.get_mut(&geometry.pane) {
                let content = content_rect(geometry.rect);
                pane.resize(content.width, content.height)?;
            }
        }
        Ok(())
    }

    fn switch_workspace(&mut self, workspace: usize) -> Result<EventResult> {
        let previous = self.workspaces.clone();
        if !self.workspaces.switch(workspace) {
            return Ok(EventResult::Continue);
        }
        if self.layout().is_empty()
            && let Err(error) = self.spawn_shell_in_empty_workspace()
        {
            self.workspaces = previous;
            self.notice = Some(format!("Could not open workspace: {error:#}"));
            return Ok(EventResult::Redraw);
        }
        self.resize_visible_panes()?;
        Ok(EventResult::Redraw)
    }

    fn move_to_workspace(&mut self, workspace: usize) -> Result<EventResult> {
        let previous = self.workspaces.clone();
        let moved = self
            .workspaces
            .move_focused_to(workspace, self.selected_split)?;
        if moved.is_none() {
            return Ok(EventResult::Continue);
        }
        if self.layout().is_empty()
            && let Err(error) = self.spawn_shell_in_empty_workspace()
        {
            self.workspaces = previous;
            self.notice = Some(format!("Could not move pane: {error:#}"));
            return Ok(EventResult::Redraw);
        }
        self.resize_visible_panes()?;
        Ok(EventResult::Redraw)
    }

    fn spawn_shell_in_empty_workspace(&mut self) -> Result<PaneId> {
        if !self.layout().is_empty() {
            anyhow::bail!("cannot initialize a non-empty workspace");
        }
        let pane_id = PaneId(self.next_pane_id);
        let content = content_rect(pane_area(self.terminal_bounds));
        let pane = Pane::spawn(
            pane_id,
            &self.shell,
            Some(&self.cwd),
            content.width,
            content.height,
            self.pty_tx.clone(),
        )?;
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        self.workspaces.active_mut().layout = TilingTree::new(pane_id);
        self.panes.insert(pane_id, pane);
        Ok(pane_id)
    }

    fn layout(&self) -> &TilingTree {
        &self.workspaces.active().layout
    }

    fn layout_mut(&mut self) -> &mut TilingTree {
        &mut self.workspaces.active_mut().layout
    }

    fn fullscreen(&self) -> Option<PaneId> {
        self.workspaces.active().fullscreen
    }

    fn apply_config(&mut self, config: &Config) {
        self.shell = CommandSpec::new(config.general.shell.clone(), std::iter::empty::<String>());
        self.selected_split = config.general.default_split.into();
        self.resize_step = config.general.resize_step;
        self.launcher = Launcher::new(&config.launcher);
        self.notice = Some("Configuration reloaded".to_owned());
    }

    fn mode_label(&self) -> Option<&'static str> {
        match self.input_mode {
            InputMode::Normal => None,
            InputMode::Resize => Some(" RESIZE "),
            InputMode::ConfirmReplace => Some(" Replace focused pane? [y/N] "),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum InputMode {
    #[default]
    Normal,
    Resize,
    ConfirmReplace,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EventResult {
    Continue,
    Redraw,
    ReloadConfig,
    Exit,
}

pub fn run() -> Result<()> {
    let config = Config::load()?;
    let mut renderer = TerminalRenderer::enter(&config.appearance)?;
    let (columns, rows) = crossterm::terminal::size().context("failed to read terminal size")?;
    let (app_tx, app_rx) = mpsc::sync_channel(128);
    let (pty_tx, pty_rx) = mpsc::sync_channel(128);
    let (launcher_tx, launcher_rx) = mpsc::sync_channel(128);
    let _pty_forwarder = spawn_pty_forwarder(pty_rx, app_tx.clone())?;
    let _launcher_forwarder = spawn_launcher_forwarder(launcher_rx, app_tx.clone())?;
    let mut input_thread = None;
    let mut signal_thread = None;
    let result = (|| {
        input_thread = Some(InputThread::spawn(app_tx.clone())?);
        signal_thread = Some(SignalThread::spawn(app_tx)?);
        let mut app = App::new(columns, rows, pty_tx, launcher_tx, &config)?;
        render(&app, &mut renderer)?;
        event_loop(&mut app, &mut renderer, &app_rx)
    })();
    drop(app_rx);
    drop(signal_thread);
    drop(input_thread);
    result
}

fn event_loop(
    app: &mut App,
    renderer: &mut TerminalRenderer,
    events: &Receiver<AppEvent>,
) -> Result<()> {
    loop {
        let first = events.recv().context("application event channel closed")?;
        let started = std::time::Instant::now();
        let mut pending = Some(first);
        let mut redraw = false;
        for _ in 0..64 {
            let Some(event) = pending.take().or_else(|| events.try_recv().ok()) else {
                break;
            };
            let result = match event {
                AppEvent::Terminal(event) => app.handle_terminal(event)?,
                AppEvent::Pty(event) => app.handle_pty(event)?,
                AppEvent::Launcher(event) => app.handle_launcher_event(event)?,
                AppEvent::TerminalError(error) => {
                    return Err(anyhow::anyhow!("terminal input failed: {error}"));
                }
                AppEvent::Shutdown => EventResult::Exit,
            };

            match result {
                EventResult::Continue => {}
                EventResult::Redraw => redraw = true,
                EventResult::ReloadConfig => {
                    match Config::load() {
                        Ok(config) => {
                            app.apply_config(&config);
                            renderer.update_appearance(&config.appearance)?;
                        }
                        Err(error) => {
                            app.notice = Some(format!("Configuration reload failed: {error}"))
                        }
                    }
                    redraw = true;
                }
                EventResult::Exit => return Ok(()),
            }
            if let Some(text) = app.pending_clipboard.take()
                && let Err(error) = renderer.copy_to_clipboard(&text)
            {
                app.notice = Some(format!("Clipboard copy failed: {error}"));
                redraw = true;
            }
            if started.elapsed() >= Duration::from_millis(8) {
                break;
            }
        }
        if redraw {
            render(app, renderer)?;
        }
    }
}

fn render(app: &App, renderer: &mut TerminalRenderer) -> Result<()> {
    let visible = visible_layout(
        app.layout(),
        app.fullscreen(),
        pane_area(app.terminal_bounds),
        MINIMUM_PANE_SIZE,
    );
    renderer.draw_panes(
        &app.panes,
        &visible,
        app.layout().focused(),
        app.terminal_bounds,
        RenderOverlays {
            mode_label: app.mode_label(),
            launcher: app.launcher.is_open().then_some(&app.launcher),
            notice: app.notice.as_deref(),
            workspace_bar: Some(WorkspaceBar {
                active: app.workspaces.active_index(),
                open: app.workspaces.open_map(),
            }),
            split_orientation: Some(app.selected_split),
        },
    )
}

fn content_rect(rect: Rect) -> Rect {
    Rect::new(
        rect.x.saturating_add(1),
        rect.y.saturating_add(1),
        rect.width.saturating_sub(2).max(1),
        rect.height.saturating_sub(2).max(1),
    )
}

fn pane_area(bounds: Rect) -> Rect {
    let status_height = u16::from(bounds.height > 0);
    Rect::new(
        bounds.x,
        bounds.y.saturating_add(status_height),
        bounds.width,
        bounds.height.saturating_sub(status_height),
    )
}

fn visible_layout(
    tree: &TilingTree,
    fullscreen: Option<PaneId>,
    bounds: Rect,
    minimum: MinimumSize,
) -> Vec<PaneGeometry> {
    let mut layout = match fullscreen.filter(|pane| tree.contains(*pane)) {
        Some(pane) => vec![PaneGeometry { pane, rect: bounds }],
        None => tree.layout(bounds, minimum),
    };
    share_stacked_pane_borders(&mut layout);
    layout
}

fn share_stacked_pane_borders(layout: &mut [PaneGeometry]) {
    let original = layout.to_vec();
    for geometry in layout {
        let bottom = geometry.rect.y.saturating_add(geometry.rect.height);
        let shares_boundary = original.iter().any(|candidate| {
            candidate.pane != geometry.pane
                && candidate.rect.y == bottom
                && ranges_overlap(
                    geometry.rect.x,
                    geometry.rect.width,
                    candidate.rect.x,
                    candidate.rect.width,
                )
        });
        if shares_boundary {
            geometry.rect.height = geometry.rect.height.saturating_add(1);
        }
    }
}

fn ranges_overlap(
    first_start: u16,
    first_length: u16,
    second_start: u16,
    second_length: u16,
) -> bool {
    let first_end = first_start.saturating_add(first_length);
    let second_end = second_start.saturating_add(second_length);
    first_start < second_end && second_start < first_end
}

fn pane_at(layout: &[PaneGeometry], column: u16, row: u16) -> Option<PaneId> {
    layout
        .iter()
        .find(|geometry| geometry.rect.contains_point(column, row))
        .map(|geometry| geometry.pane)
}

fn is_host_copy_shortcut(key: KeyEvent) -> bool {
    !matches!(key.kind, KeyEventKind::Release)
        && key
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Char('c' | 'C'))
}

fn is_host_paste_shortcut(key: KeyEvent) -> bool {
    !matches!(key.kind, KeyEventKind::Release)
        && key
            .modifiers
            .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Char('v' | 'V'))
}

fn pane_content_at(layout: &[PaneGeometry], column: u16, row: u16) -> Option<(PaneId, Position)> {
    layout.iter().find_map(|geometry| {
        pane_position(layout, geometry.pane, column, row, false)
            .map(|position| (geometry.pane, position))
    })
}

fn pane_position(
    layout: &[PaneGeometry],
    pane: PaneId,
    column: u16,
    row: u16,
    clamp: bool,
) -> Option<Position> {
    let geometry = layout.iter().find(|geometry| geometry.pane == pane)?;
    let width = geometry.rect.width.saturating_sub(2);
    let height = geometry.rect.height.saturating_sub(2);
    if width == 0 || height == 0 {
        return None;
    }
    let left = geometry.rect.x.saturating_add(1);
    let top = geometry.rect.y.saturating_add(1);
    let right = left.saturating_add(width.saturating_sub(1));
    let bottom = top.saturating_add(height.saturating_sub(1));
    if !clamp && (column < left || column > right || row < top || row > bottom) {
        return None;
    }
    Some(Position {
        row: row.clamp(top, bottom).saturating_sub(top),
        col: column.clamp(left, right).saturating_sub(left),
    })
}

fn spawn_pty_forwarder(
    pty_rx: Receiver<PtyEvent>,
    app_tx: SyncSender<AppEvent>,
) -> Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tili-pty-events".to_owned())
        .spawn(move || {
            while let Ok(event) = pty_rx.recv() {
                if app_tx.send(AppEvent::Pty(event)).is_err() {
                    break;
                }
            }
        })
        .context("failed to start PTY event thread")
}

fn spawn_launcher_forwarder(
    launcher_rx: Receiver<LauncherEvent>,
    app_tx: SyncSender<AppEvent>,
) -> Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("tili-launcher-events".to_owned())
        .spawn(move || {
            while let Ok(event) = launcher_rx.recv() {
                if app_tx.send(AppEvent::Launcher(event)).is_err() {
                    break;
                }
            }
        })
        .context("failed to start launcher event thread")
}

struct InputThread {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl InputThread {
    fn spawn(event_tx: SyncSender<AppEvent>) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("tili-input".to_owned())
            .spawn(move || {
                while !thread_stop.load(Ordering::Relaxed) {
                    match event::poll(Duration::from_millis(100)) {
                        Ok(true) => match event::read() {
                            Ok(event) => {
                                if event_tx.send(AppEvent::Terminal(event)).is_err() {
                                    break;
                                }
                            }
                            Err(error) => {
                                let _ = event_tx.send(AppEvent::TerminalError(error.to_string()));
                                break;
                            }
                        },
                        Ok(false) => {}
                        Err(error) => {
                            let _ = event_tx.send(AppEvent::TerminalError(error.to_string()));
                            break;
                        }
                    }
                }
            })
            .context("failed to start terminal input thread")?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct SignalThread {
    handle: SignalHandle,
    thread: Option<JoinHandle<()>>,
}

impl SignalThread {
    fn spawn(event_tx: SyncSender<AppEvent>) -> Result<Self> {
        let mut signals = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT])
            .context("failed to register signal handlers")?;
        let handle = signals.handle();
        let thread = thread::Builder::new()
            .name("tili-signals".to_owned())
            .spawn(move || {
                if signals.forever().next().is_some() {
                    let _ = event_tx.send(AppEvent::Shutdown);
                }
            })
            .context("failed to start signal thread")?;
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for SignalThread {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::config::{BuiltinMode, LauncherConfig, LauncherModeConfig};
    use crossterm::event::{MouseButton, MouseEvent};

    #[test]
    fn failed_spawns_preserve_panes_focus_fullscreen_and_workspaces() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-app-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        let focused = app.layout().focused();
        app.workspaces.active_mut().fullscreen = focused;
        let layout = app.layout().clone();
        let invalid = CommandSpec::new("/nonexistent/tili-test-command", Vec::<String>::new());
        assert!(app.replace_focused(invalid.clone()).is_err());
        assert!(
            app.execute_launcher_action(LauncherAction::OpenPane {
                command: invalid.program.clone(),
                args: vec![],
            })
            .is_err()
        );
        app.shell = invalid.clone();
        assert_eq!(app.dispatch(Action::OpenPane).unwrap(), EventResult::Redraw);
        assert!(app.notice.as_ref().unwrap().contains("Could not open pane"));
        app.dispatch(Action::OpenPaneInWorkingDirectory).unwrap();
        app.pending_replace = Some(invalid);
        app.handle_replace_confirmation(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        app.switch_workspace(1).unwrap();
        app.move_to_workspace(1).unwrap();
        assert_eq!(app.workspaces.active_index(), 0);
        assert_eq!(
            app.workspaces
                .open_map()
                .iter()
                .filter(|open| **open)
                .count(),
            1
        );
        assert_eq!(app.layout(), &layout);
        assert_eq!(app.fullscreen(), focused);
        assert_eq!(app.panes.len(), 1);
        assert!(
            app.panes
                .get_mut(&focused.unwrap())
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn saturated_forwarders_stop_when_the_application_receiver_closes() {
        let (app_tx, app_rx) = mpsc::sync_channel(1);
        let (pty_tx, pty_rx) = mpsc::sync_channel(1);
        let forwarder = spawn_pty_forwarder(pty_rx, app_tx).unwrap();
        let producer = thread::spawn(move || {
            while pty_tx
                .send(PtyEvent::Output(PaneId(1), vec![0; 8192]))
                .is_ok()
            {}
        });
        thread::sleep(Duration::from_millis(20));
        drop(app_rx);
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            forwarder.join().unwrap();
            producer.join().unwrap();
            done_tx.send(()).unwrap();
        });
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn fullscreen_layout_preserves_and_restores_the_tree() {
        let mut tree = TilingTree::new(PaneId(1));
        tree.split_focused(PaneId(2), Orientation::Horizontal)
            .unwrap();
        let bounds = Rect::new(0, 0, 100, 30);
        let tiled = visible_layout(&tree, None, bounds, MINIMUM_PANE_SIZE);

        let fullscreen = visible_layout(&tree, Some(PaneId(2)), bounds, MINIMUM_PANE_SIZE);
        assert_eq!(
            fullscreen,
            vec![PaneGeometry {
                pane: PaneId(2),
                rect: bounds
            }]
        );
        assert_eq!(tree.pane_count(), 2);
        assert_eq!(
            visible_layout(&tree, None, bounds, MINIMUM_PANE_SIZE),
            tiled
        );
    }

    #[test]
    fn stacked_panes_share_one_border_row() {
        let mut tree = TilingTree::new(PaneId(1));
        tree.split_focused(PaneId(2), Orientation::Vertical)
            .unwrap();

        let layout = visible_layout(&tree, None, Rect::new(0, 0, 80, 24), MINIMUM_PANE_SIZE);
        let upper = layout
            .iter()
            .find(|geometry| geometry.pane == PaneId(1))
            .unwrap();
        let lower = layout
            .iter()
            .find(|geometry| geometry.pane == PaneId(2))
            .unwrap();

        assert_eq!(upper.rect.y + upper.rect.height - 1, lower.rect.y);
    }

    #[test]
    fn side_by_side_panes_keep_separate_border_columns() {
        let mut tree = TilingTree::new(PaneId(1));
        tree.split_focused(PaneId(2), Orientation::Horizontal)
            .unwrap();

        let layout = visible_layout(&tree, None, Rect::new(0, 0, 80, 24), MINIMUM_PANE_SIZE);
        let left = layout
            .iter()
            .find(|geometry| geometry.pane == PaneId(1))
            .unwrap();
        let right = layout
            .iter()
            .find(|geometry| geometry.pane == PaneId(2))
            .unwrap();

        assert_eq!(left.rect.x + left.rect.width, right.rect.x);
    }

    #[test]
    fn pane_hit_testing_uses_geometry_and_rejects_positions_outside_panes() {
        let layout = vec![
            PaneGeometry {
                pane: PaneId(1),
                rect: Rect::new(0, 0, 40, 20),
            },
            PaneGeometry {
                pane: PaneId(2),
                rect: Rect::new(40, 0, 40, 20),
            },
        ];
        assert_eq!(pane_at(&layout, 2, 2), Some(PaneId(1)));
        assert_eq!(pane_at(&layout, 60, 2), Some(PaneId(2)));
        assert_eq!(pane_at(&layout, 90, 2), None);
    }

    #[test]
    fn launcher_open_action_creates_and_focuses_a_pane() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-app-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();

        app.execute_launcher_action(LauncherAction::OpenPane {
            command: config.general.shell,
            args: Vec::new(),
        })
        .unwrap();

        assert_eq!(app.layout().pane_count(), 2);
        assert_eq!(app.layout().focused(), Some(PaneId(2)));
        assert!(app.panes.contains_key(&PaneId(2)));
    }

    #[test]
    fn launcher_arrow_keys_switch_modes_and_wrap() {
        let configuration = Config {
            launcher: LauncherConfig {
                default_mode: "ssh".to_owned(),
                width_percent: 60,
                max_results: 10,
                timeout_ms: 1_000,
                modes: vec![
                    LauncherModeConfig {
                        name: "ssh".to_owned(),
                        command: None,
                        builtin: Some(BuiltinMode::Ssh),
                    },
                    LauncherModeConfig {
                        name: "examples".to_owned(),
                        command: Some(PathBuf::from("/bin/true")),
                        builtin: None,
                    },
                ],
            },
            ..Config::default_for(Path::new("/tmp/tili-arrow-test"))
        };
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &configuration).unwrap();
        app.launcher.open(&app.launcher_tx);
        assert_eq!(app.launcher.mode_name(), "ssh");
        app.handle_launcher_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.launcher.mode_name(), "examples");
        app.handle_launcher_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
        assert_eq!(app.launcher.mode_name(), "ssh");
        app.handle_launcher_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
        assert_eq!(app.launcher.mode_name(), "examples");
    }

    #[test]
    fn failed_pane_keeps_its_output_while_successful_pane_closes() {
        let (pty_tx, pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-exit-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        let cwd = app.cwd.clone();

        app.execute_launcher_action(LauncherAction::OpenPane {
            command: "/bin/sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf 'SSH_TEST_FAILURE\\n'; exit 255".to_owned(),
            ],
        })
        .unwrap();
        let failed = app.layout().focused().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !app.panes.get(&failed).unwrap().has_exited() {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for failed pane");
            app.handle_pty(pty_rx.recv_timeout(remaining).unwrap())
                .unwrap();
        }
        let screen = app.panes.get(&failed).unwrap().screen();
        let failed_screen = screen.contents();
        assert!(failed_screen.contains("SSH_TEST_FAILURE"));
        assert!(failed_screen.contains("Exited with code 255"));
        let size = screen.size();
        assert!((0..size.rows).any(|row| {
            (0..size.cols).any(|column| {
                screen.cell(row, column).is_some_and(|cell| {
                    cell.has_contents()
                        && cell.attrs().fg_color == tastty_core::Color::Rgb(0xcc, 0x34, 0x2b)
                })
            })
        }));
        assert!(app.notice.is_none());

        app.close_pane(failed).unwrap();
        app.execute_launcher_action(LauncherAction::OpenPane {
            command: "/bin/true".to_owned(),
            args: Vec::new(),
        })
        .unwrap();
        let successful = app.layout().focused().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.panes.contains_key(&successful) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for successful pane"
            );
            app.handle_pty(pty_rx.recv_timeout(remaining).unwrap())
                .unwrap();
        }
        assert_eq!(app.layout().pane_count(), 1);

        let shell = app.open_pane(app.shell.clone(), cwd).unwrap();
        app.panes
            .get_mut(&shell)
            .unwrap()
            .write_input(b"false\nexit\n")
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while app.panes.contains_key(&shell) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for shell to exit");
            app.handle_pty(pty_rx.recv_timeout(remaining).unwrap())
                .unwrap();
        }
        assert_eq!(app.layout().pane_count(), 1);
    }

    #[test]
    fn mouse_click_does_not_change_focus_until_pointer_motion_occurs() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-mouse-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.open_pane(app.shell.clone(), app.cwd.clone()).unwrap();
        app.dispatch(Action::Focus(crate::tiling::Direction::Left))
            .unwrap();
        assert_eq!(app.layout().focused(), Some(PaneId(1)));

        let pointer = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 60,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };
        app.handle_terminal(TerminalEvent::Mouse(pointer)).unwrap();
        app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            ..pointer
        }))
        .unwrap();
        assert_eq!(app.layout().focused(), Some(PaneId(1)));
        assert_eq!(app.selection, None);

        app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            ..pointer
        }))
        .unwrap();
        assert_eq!(app.layout().focused(), Some(PaneId(2)));
    }

    #[test]
    fn closing_a_pane_resizes_the_surviving_pty_to_its_expanded_area() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-close-resize-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        let second = app.open_pane(app.shell.clone(), app.cwd.clone()).unwrap();
        let narrow = app.panes[&PaneId(1)].screen().size().cols;

        app.close_pane(second).unwrap();

        let expanded = app.panes[&PaneId(1)].screen().size();
        assert!(expanded.cols > narrow);
        assert_eq!(expanded.cols, 78);
        assert_eq!(expanded.rows, 21);
    }

    #[test]
    fn mouse_drag_selects_text_and_queues_it_for_the_host_clipboard() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"hello")
            .unwrap();

        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            let column = if kind == MouseEventKind::Down(MouseButton::Left) {
                1
            } else {
                5
            };
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column,
                row: 2,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }

        assert_eq!(app.pending_clipboard.as_deref(), Some("hello"));
        assert_eq!(app.selection, Some(PaneId(1)));
        assert_eq!(
            app.panes[&PaneId(1)].selected_text().as_deref(),
            Some("hello")
        );

        app.handle_pty(PtyEvent::Output(PaneId(1), b"\r\nlater".to_vec()))
            .unwrap();
        assert_eq!(app.selection, Some(PaneId(1)));

        app.pending_clipboard = None;
        app.handle_terminal(TerminalEvent::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )))
        .unwrap();
        assert_eq!(app.pending_clipboard.as_deref(), Some("hello"));
    }

    #[test]
    fn mouse_drag_copies_text_from_alternate_screen() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"\x1b[?1049hhello  \r\nworld")
            .unwrap();

        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 1, 2),
            (MouseEventKind::Drag(MouseButton::Left), 7, 3),
            (MouseEventKind::Up(MouseButton::Left), 7, 3),
        ] {
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }

        assert_eq!(app.pending_clipboard.as_deref(), Some("hello\nworld"));
        assert_eq!(app.selection, Some(PaneId(1)));
    }

    #[test]
    fn mouse_drag_across_rows_queues_multiline_text() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"first\r\nsecond")
            .unwrap();

        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 1, 2),
            (MouseEventKind::Drag(MouseButton::Left), 6, 3),
            (MouseEventKind::Up(MouseButton::Left), 6, 3),
        ] {
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }

        assert_eq!(app.pending_clipboard.as_deref(), Some("first\nsecond"));
    }

    #[test]
    fn mouse_drag_across_rows_queues_unicode_and_punctuation() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output("α中🙂\u{a0}\r\ncafé <>&[]{}".as_bytes())
            .unwrap();

        for (kind, column, row) in [
            (MouseEventKind::Down(MouseButton::Left), 1, 2),
            (MouseEventKind::Drag(MouseButton::Left), 12, 3),
            (MouseEventKind::Up(MouseButton::Left), 12, 3),
        ] {
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }

        assert_eq!(
            app.pending_clipboard.as_deref(),
            Some("α中🙂\ncafé <>&[]{}")
        );
    }

    #[test]
    fn empty_mouse_selection_is_discarded() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"text")
            .unwrap();

        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column: 20,
                row: 8,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }

        assert_eq!(app.selection, None);
        assert_eq!(app.pending_clipboard, None);
    }

    #[test]
    fn mouse_drag_can_begin_in_padding_and_clamps_copy_to_text() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"hello")
            .unwrap();

        app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();
        app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 1,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();

        assert_eq!(app.pending_clipboard.as_deref(), Some("hello"));
    }

    #[test]
    fn consecutive_mouse_selections_replace_the_pending_clipboard_text() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-selection-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.panes
            .get_mut(&PaneId(1))
            .unwrap()
            .process_output(b"first second")
            .unwrap();

        drag_selection(&mut app, 1, 5);
        assert_eq!(app.pending_clipboard.take().as_deref(), Some("first"));

        drag_selection(&mut app, 7, 12);
        assert_eq!(app.pending_clipboard.take().as_deref(), Some("second"));
    }

    fn drag_selection(app: &mut App, start_column: u16, end_column: u16) {
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), start_column),
            (MouseEventKind::Drag(MouseButton::Left), end_column),
            (MouseEventKind::Up(MouseButton::Left), end_column),
        ] {
            app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
                kind,
                column,
                row: 2,
                modifiers: KeyModifiers::NONE,
            }))
            .unwrap();
        }
    }

    #[test]
    fn host_copy_and_paste_shortcuts_are_reserved() {
        assert!(is_host_copy_shortcut(KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )));
        assert!(is_host_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        )));
        assert!(!is_host_paste_shortcut(KeyEvent::new(
            KeyCode::Char('v'),
            KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn wheel_scrolls_the_pane_under_the_pointer_without_changing_focus() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-scroll-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        let second = app.open_pane(app.shell.clone(), app.cwd.clone()).unwrap();
        app.dispatch(Action::Focus(crate::tiling::Direction::Left))
            .unwrap();
        for line in 0..40 {
            app.panes
                .get_mut(&second)
                .unwrap()
                .process_output(format!("line {line}\r\n").as_bytes())
                .unwrap();
        }

        app.handle_terminal(TerminalEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 60,
            row: 10,
            modifiers: KeyModifiers::NONE,
        }))
        .unwrap();

        assert_eq!(app.layout().focused(), Some(PaneId(1)));
        assert_eq!(app.panes.get(&second).unwrap().screen().scrollback(), 3);
        assert_eq!(app.panes.get(&PaneId(1)).unwrap().screen().scrollback(), 0);
    }

    #[test]
    fn closing_last_workspace_pane_returns_then_final_close_exits() {
        let (pty_tx, _pty_rx) = mpsc::sync_channel(128);
        let (launcher_tx, _launcher_rx) = mpsc::sync_channel(128);
        let config = Config::default_for(std::path::Path::new("/tmp/tili-close-test"));
        let mut app = App::new(80, 24, pty_tx, launcher_tx, &config).unwrap();
        app.switch_workspace(1).unwrap();
        let second_workspace_pane = app.layout().focused().unwrap();
        assert_eq!(app.workspaces.open_count(), 2);

        assert_eq!(
            app.close_pane(second_workspace_pane).unwrap(),
            EventResult::Redraw
        );
        assert_eq!(app.workspaces.active_index(), 0);
        assert_eq!(app.panes.len(), 1);

        assert_eq!(app.close_pane(PaneId(1)).unwrap(), EventResult::Exit);
        assert!(app.panes.is_empty());
    }
}
