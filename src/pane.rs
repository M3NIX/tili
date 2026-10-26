use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use anyhow::{Context, Result};
use crossterm::event::MouseEvent;
use tastty_core::host_reply::auto_reply_bytes;
use tastty_core::screen::SelectionMode;
use tastty_core::{
    HostProfile, MouseEncoder, Parser, Position, Screen, TerminalMode, TerminalSize,
};

use crate::pty::{CommandSpec, PtyEvent, PtySession, pty_size};
use crate::tiling::PaneId;

pub struct Pane {
    id: PaneId,
    terminal: Parser,
    host_profile: Arc<HostProfile>,
    pty: PtySession,
    columns: u16,
    rows: u16,
    exited: bool,
}

impl Pane {
    pub fn spawn(
        id: PaneId,
        command: &CommandSpec,
        cwd: Option<&Path>,
        columns: u16,
        rows: u16,
        event_tx: SyncSender<PtyEvent>,
    ) -> Result<Self> {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let mut host_profile = HostProfile::default();
        host_profile.default_fg = tastty_core::Color::Default;
        host_profile.default_bg = tastty_core::Color::Default;
        let host_profile = Arc::new(host_profile);
        let terminal = Parser::with_profile(
            TerminalSize {
                rows,
                cols: columns,
            },
            10_000,
            Arc::clone(&host_profile),
        );
        let pty = PtySession::spawn(id, command, cwd, pty_size(columns, rows), event_tx)?;
        Ok(Self {
            id,
            terminal,
            host_profile,
            pty,
            columns,
            rows,
            exited: false,
        })
    }

    pub fn id(&self) -> PaneId {
        self.id
    }

    pub fn process_output(&mut self, bytes: &[u8]) -> Result<()> {
        self.terminal.process(bytes);
        for event in self.terminal.screen_mut().drain_events() {
            // The host's inherited defaults are unknown. Do not let the reply
            // encoder misreport Color::Default as black.
            if matches!(
                event,
                tastty_core::ScreenEvent::ColorQuery {
                    color: tastty_core::Color::Default,
                    ..
                }
            ) {
                continue;
            }
            if let Some(reply) = auto_reply_bytes(&event, &self.host_profile) {
                self.pty.write_all(&reply)?;
            }
        }
        Ok(())
    }

    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        if self.exited {
            return Ok(());
        }
        self.terminal.screen_mut().scroll_reset();
        self.pty.write_all(bytes)
    }

    pub fn write_mouse(&mut self, event: MouseEvent) -> Result<bool> {
        if self.exited {
            return Ok(false);
        }
        let mut encoder = MouseEncoder::new();
        encoder.sync(self.terminal.screen());
        let event = tastty_core::MouseEvent::from(event);
        let Some(bytes) = encoder.encode_mouse(&event) else {
            return Ok(false);
        };
        self.pty.write_all(&bytes)?;
        Ok(true)
    }

    pub fn reports_mouse(&self) -> bool {
        let mut encoder = MouseEncoder::new();
        encoder.sync(self.terminal.screen());
        encoder.mouse_enabled()
    }

    pub fn scroll_up(&mut self, rows: usize) -> bool {
        let before = self.terminal.screen().scrollback();
        self.terminal.screen_mut().scroll_up(rows);
        self.terminal.screen().scrollback() != before
    }

    pub fn scroll_down(&mut self, rows: usize) -> bool {
        let before = self.terminal.screen().scrollback();
        self.terminal.screen_mut().scroll_down(rows);
        self.terminal.screen().scrollback() != before
    }

    pub fn selection_start(&mut self, position: Position) -> bool {
        let Some(position) = self.terminal.screen().visible_to_absolute(position) else {
            return false;
        };
        self.terminal
            .screen_mut()
            .selection_start(position, SelectionMode::Linear);
        true
    }

    pub fn selection_extend(&mut self, position: Position) -> bool {
        let Some(position) = self.terminal.screen().visible_to_absolute(position) else {
            return false;
        };
        self.terminal.screen_mut().selection_extend(position);
        true
    }

    pub fn selection_clear(&mut self) {
        self.terminal.screen_mut().selection_clear();
    }

    pub fn selection_contains(&self, position: Position) -> bool {
        let screen = self.terminal.screen();
        let Some(position) = screen.visible_to_absolute(position) else {
            return false;
        };
        screen
            .selection_range()
            .is_some_and(|range| position >= range.start && position <= range.end)
    }

    pub fn selected_text(&self) -> Option<String> {
        let screen = self.terminal.screen();
        let text = if screen.mode(TerminalMode::AlternateScreen) {
            alternate_screen_selected_text(screen)
        } else {
            screen.selected_text()
        };
        text.filter(|text| text.chars().any(|character| !character.is_whitespace()))
    }

    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        let columns = columns.max(1);
        let rows = rows.max(1);
        if self.columns == columns && self.rows == rows {
            return Ok(());
        }
        self.selection_clear();
        let screen = self.terminal.screen();
        let in_band_resize = screen.mode(TerminalMode::InBandResize);
        let pixel_cell = screen.pixel_cell_size();
        self.pty.resize_with_pixels(
            columns,
            rows,
            columns.saturating_mul(pixel_cell.width),
            rows.saturating_mul(pixel_cell.height),
        )?;
        self.terminal.screen_mut().set_size(TerminalSize {
            rows,
            cols: columns,
        });
        if in_band_resize {
            self.pty.write_all(
                format!(
                    "\x1b[48;{rows};{columns};{};{}t",
                    rows.saturating_mul(pixel_cell.height),
                    columns.saturating_mul(pixel_cell.width)
                )
                .as_bytes(),
            )?;
        }
        self.columns = columns;
        self.rows = rows;
        Ok(())
    }

    pub fn screen(&self) -> &Screen {
        self.terminal.screen()
    }

    pub fn current_working_directory(&self) -> Option<PathBuf> {
        self.pty.current_working_directory()
    }

    pub fn try_wait(&mut self) -> Result<Option<portable_pty::ExitStatus>> {
        self.pty
            .try_wait()
            .with_context(|| format!("failed to poll pane {}", self.id))
    }

    pub fn mark_exited(&mut self, status: &str) {
        self.exited = true;
        let _ = self.process_output(
            format!(
                "\r\n\x1b[38;2;204;52;43m[tili: {status}; Alt+Backspace closes this pane]\x1b[0m\r\n"
            )
            .as_bytes(),
        );
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }
}

fn alternate_screen_selected_text(screen: &Screen) -> Option<String> {
    let range = screen.selection_range()?;
    let start = screen.absolute_to_visible(range.start)?;
    let end = screen.absolute_to_visible(range.end)?;
    Some(
        screen
            .contents_between(start, end)
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
