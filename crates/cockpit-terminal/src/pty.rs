//! Cross-platform PTY wrapper.
//!
//! Live PTY behavior is tested only with the `integration` feature. The public
//! wrapper is small on purpose: spawn, write, resize, read, and terminate.

use std::{
    io::{self, Read, Write},
    thread,
    time::{Duration, Instant},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

use crate::io_thread::{ReaderThread, spawn_reader_thread};
use crate::zellij::CommandSpec;

/// PTY dimensions in terminal cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyDimensions {
    pub rows: u16,
    pub cols: u16,
}

impl PtyDimensions {
    /// Create dimensions, clamping both axes to at least one cell.
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: rows.max(1),
            cols: cols.max(1),
        }
    }

    fn to_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl Default for PtyDimensions {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// A running PTY child process.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
}

impl PtySession {
    /// Spawn a command inside a native PTY.
    pub fn spawn(command: &CommandSpec, dimensions: PtyDimensions) -> Result<Self, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(dimensions.to_pty_size())
            .map_err(PtyError::Open)?;

        let mut builder = CommandBuilder::new(&command.program);
        for arg in &command.args {
            builder.arg(arg);
        }

        let child = pair.slave.spawn_command(builder).map_err(PtyError::Spawn)?;
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(PtyError::CloneReader)?;
        let writer = pair.master.take_writer().map_err(PtyError::TakeWriter)?;

        Ok(Self {
            master: pair.master,
            child,
            reader: Some(reader),
            writer,
        })
    }

    /// Write input bytes to the PTY.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        self.writer.write_all(bytes).map_err(PtyError::Write)?;
        self.writer.flush().map_err(PtyError::Write)
    }

    /// Try to read once from the PTY. This call may block until output is
    /// available, depending on the platform PTY backend.
    pub fn read_once(&mut self, buffer: &mut [u8]) -> Result<usize, PtyError> {
        let reader = self.reader.as_mut().ok_or(PtyError::ReaderTaken)?;
        reader.read(buffer).map_err(PtyError::Read)
    }

    /// Move the PTY reader onto a dedicated reader thread. This can only be
    /// called once.
    pub fn spawn_reader_thread(&mut self) -> Result<ReaderThread, PtyError> {
        let reader = self.reader.take().ok_or(PtyError::ReaderTaken)?;
        Ok(spawn_reader_thread(reader))
    }

    /// Read until `needle` appears or `timeout` elapses.
    pub fn read_until(&mut self, needle: &[u8], timeout: Duration) -> Result<Vec<u8>, PtyError> {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        let mut buffer = [0; 4096];

        while Instant::now() < deadline {
            match self.read_once(&mut buffer) {
                Ok(0) => thread::sleep(Duration::from_millis(10)),
                Ok(n) => {
                    output.extend_from_slice(&buffer[..n]);
                    if output.windows(needle.len()).any(|window| window == needle) {
                        return Ok(output);
                    }
                }
                Err(PtyError::Read(err)) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(err) => return Err(err),
            }
        }

        Err(PtyError::Timeout)
    }

    /// Resize the PTY.
    pub fn resize(&mut self, dimensions: PtyDimensions) -> Result<(), PtyError> {
        self.master
            .resize(dimensions.to_pty_size())
            .map_err(PtyError::Resize)
    }

    /// Ask the child process to terminate.
    pub fn terminate(&mut self) -> Result<(), PtyError> {
        self.child.kill().map_err(PtyError::Terminate)
    }

    /// Wait for child exit.
    pub fn wait(&mut self) -> Result<(), PtyError> {
        self.child.wait().map(|_| ()).map_err(PtyError::Wait)
    }
}

/// PTY wrapper error.
#[derive(Debug, Error)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    Open(#[source] anyhow::Error),
    #[error("failed to spawn PTY command: {0}")]
    Spawn(#[source] anyhow::Error),
    #[error("failed to clone PTY reader: {0}")]
    CloneReader(#[source] anyhow::Error),
    #[error("failed to take PTY writer: {0}")]
    TakeWriter(#[source] anyhow::Error),
    #[error("failed to write to PTY: {0}")]
    Write(#[source] io::Error),
    #[error("failed to read from PTY: {0}")]
    Read(#[source] io::Error),
    #[error("failed to resize PTY: {0}")]
    Resize(#[source] anyhow::Error),
    #[error("failed to terminate PTY child: {0}")]
    Terminate(#[source] io::Error),
    #[error("failed waiting for PTY child: {0}")]
    Wait(#[source] io::Error),
    #[error("timed out waiting for PTY output")]
    Timeout,
    #[error("PTY reader has already been moved to a reader thread")]
    ReaderTaken,
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use crate::io_thread::TerminalIoEvent;

    use super::*;

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows PTY output is not reliable on GitHub Actions"
    )]
    fn starts_shell_writes_reads_resizes_and_terminates() {
        let command = if cfg!(windows) {
            CommandSpec::new("cmd.exe", Vec::<String>::new())
        } else {
            CommandSpec::new("/bin/sh", Vec::<String>::new())
        };
        let mut session = PtySession::spawn(&command, PtyDimensions::new(24, 80)).unwrap();
        let reader = session.spawn_reader_thread().unwrap();

        let command = if cfg!(windows) {
            b"echo cockpit-pty\r\n".as_slice()
        } else {
            b"printf cockpit-pty\\n\n".as_slice()
        };
        session.write_all(command).unwrap();
        let mut output = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !output
            .windows(b"cockpit-pty".len())
            .any(|window| window == b"cockpit-pty")
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for PTY output: {}",
                String::from_utf8_lossy(&output)
            );
            match reader.recv_timeout(remaining).unwrap() {
                TerminalIoEvent::Output(bytes) => output.extend(bytes),
                TerminalIoEvent::Eof => panic!("PTY closed before expected output"),
                TerminalIoEvent::ReadError(error) => panic!("PTY read failed: {error}"),
            }
        }
        assert!(String::from_utf8_lossy(&output).contains("cockpit-pty"));

        session.resize(PtyDimensions::new(30, 100)).unwrap();
        session.terminate().unwrap();
        if !cfg!(windows) {
            session.wait().unwrap();
        }
    }
}
