use std::io::{Read, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::tiling::PaneId;

#[derive(Debug)]
pub enum PtyEvent {
    Output(PaneId, Vec<u8>),
    Eof(PaneId),
    ReadError(PaneId, String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader_thread: Option<JoinHandle<()>>,
}

impl PtySession {
    pub fn spawn(
        pane: PaneId,
        command: &CommandSpec,
        cwd: Option<&Path>,
        size: PtySize,
        event_tx: SyncSender<PtyEvent>,
    ) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(size)
            .context("failed to open pseudo-terminal")?;
        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.args);
        if let Some(cwd) = cwd {
            builder.cwd(cwd);
        }

        let writer = pair
            .master
            .take_writer()
            .context("failed to open PTY writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("failed to open PTY reader")?;
        let child = pair
            .slave
            .spawn_command(builder)
            .with_context(|| format!("failed to spawn {}", command.program))?;
        drop(pair.slave);
        let mut session = Self {
            master: pair.master,
            writer,
            child,
            reader_thread: None,
        };
        let reader_thread = thread::Builder::new()
            .name(format!("tili-pty-{}", pane.0))
            .spawn(move || {
                let mut buffer = [0_u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => {
                            let _ = event_tx.send(PtyEvent::Eof(pane));
                            break;
                        }
                        Ok(length) => {
                            if event_tx
                                .send(PtyEvent::Output(pane, buffer[..length].to_vec()))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            let _ = event_tx.send(PtyEvent::ReadError(pane, error.to_string()));
                            break;
                        }
                    }
                }
            })
            .context("failed to start PTY reader thread")?;

        session.reader_thread = Some(reader_thread);
        Ok(session)
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer
            .write_all(bytes)
            .context("failed to write to PTY")?;
        self.writer.flush().context("failed to flush PTY input")
    }

    pub fn resize(&self, columns: u16, rows: u16) -> Result<()> {
        self.resize_with_pixels(columns, rows, 0, 0)
    }

    pub fn resize_with_pixels(
        &self,
        columns: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols: columns,
                pixel_width,
                pixel_height,
            })
            .context("failed to resize PTY")
    }

    pub fn size(&self) -> Result<PtySize> {
        self.master.get_size().context("failed to read PTY size")
    }

    pub fn process_id(&self) -> Option<u32> {
        self.child.process_id()
    }

    pub fn current_working_directory(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            let foreground_pid = self.master.as_raw_fd().and_then(|file_descriptor| {
                // SAFETY: `file_descriptor` is owned by the live PTY master and
                // `tcgetpgrp` does not retain it.
                let process_group = unsafe { libc::tcgetpgrp(file_descriptor) };
                (process_group > 0).then_some(process_group as u32)
            });
            foreground_pid
                .or_else(|| self.child.process_id())
                .and_then(|process_id| std::fs::read_link(format!("/proc/{process_id}/cwd")).ok())
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    pub fn try_wait(&mut self) -> Result<Option<portable_pty::ExitStatus>> {
        self.child
            .try_wait()
            .context("failed to poll child process")
    }

    pub fn terminate(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            #[cfg(unix)]
            if let Some(process_group) = self.master.process_group_leader() {
                unsafe {
                    libc::kill(-process_group, libc::SIGHUP);
                }
            }
            self.child.kill().context("failed to terminate child")?;
            let _ = self.child.wait();
        }
        Ok(())
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.terminate();
        self.reader_thread.take();
    }
}

pub fn pty_size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: columns.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}
