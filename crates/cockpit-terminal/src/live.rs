//! Live terminal — a spawned PTY wired to a `termwiz` engine.
//!
//! [`LiveTerminal`] is the production aggregate behind the terminal pane: it
//! spawns a process in a PTY, drives a reader thread that feeds output into a
//! [`TermwizEngine`], and exposes thread-safe [`snapshot`](LiveTerminal::snapshot)
//! and input/resize calls for the UI thread.
//!
//! Like the windowing harness this is genuinely non-headless I/O glue — the
//! engine and grid it drives are themselves fully unit-tested. Live behaviour
//! is covered by the `integration`-gated test below.

use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::engine::{ScreenGrid, TerminalEngine};
use crate::io_thread::TerminalIoEvent;
use crate::pty::{PtyDimensions, PtyError, PtySession};
use crate::session::TerminalStatus;
use crate::termwiz_engine::TermwizEngine;
use crate::zellij::CommandSpec;

/// Callback invoked on the reader thread after every terminal state change, so
/// the UI can schedule a redraw.
pub type WakeFn = Box<dyn Fn() + Send + 'static>;

/// Engine + lifecycle status shared between the UI and reader threads.
struct Shared {
    engine: TermwizEngine,
    status: TerminalStatus,
}

/// An immutable snapshot of the terminal for one UI frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub grid: ScreenGrid,
    pub status: TerminalStatus,
}

/// A spawned PTY process whose output feeds a `termwiz` engine on a thread.
pub struct LiveTerminal {
    pty: PtySession,
    shared: Arc<Mutex<Shared>>,
    dimensions: PtyDimensions,
    _reader: JoinHandle<()>,
}

impl LiveTerminal {
    /// Spawn `command` in a PTY of `dimensions` and start feeding its output
    /// into a `termwiz` engine on a background thread. `wake` is called after
    /// every state change.
    pub fn spawn(
        command: &CommandSpec,
        dimensions: PtyDimensions,
        wake: WakeFn,
    ) -> Result<Self, PtyError> {
        let mut pty = PtySession::spawn(command, dimensions)?;
        let reader_thread = pty.spawn_reader_thread()?;
        let shared = Arc::new(Mutex::new(Shared {
            engine: TermwizEngine::new(dimensions.cols as usize, dimensions.rows as usize),
            status: TerminalStatus::Running,
        }));

        let thread_shared = Arc::clone(&shared);
        let relay = thread::spawn(move || {
            while let Ok(event) = reader_thread.recv() {
                let finished = apply_event(&thread_shared, event);
                wake();
                if finished {
                    break;
                }
            }
        });

        Ok(Self {
            pty,
            shared,
            dimensions,
            _reader: relay,
        })
    }

    /// Forward input bytes to the PTY.
    pub fn send_input(&mut self, bytes: &[u8]) -> Result<(), PtyError> {
        self.pty.write_all(bytes)
    }

    /// Resize the PTY and the engine grid. A no-op when `dimensions` is
    /// unchanged.
    pub fn resize(&mut self, dimensions: PtyDimensions) -> Result<(), PtyError> {
        if dimensions == self.dimensions {
            return Ok(());
        }
        self.pty.resize(dimensions)?;
        if let Ok(mut shared) = self.shared.lock() {
            shared
                .engine
                .resize(dimensions.cols as usize, dimensions.rows as usize);
        }
        self.dimensions = dimensions;
        Ok(())
    }

    /// Current PTY dimensions.
    pub fn dimensions(&self) -> PtyDimensions {
        self.dimensions
    }

    /// Snapshot the grid and lifecycle status for one UI frame.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let shared = match self.shared.lock() {
            Ok(shared) => shared,
            Err(poisoned) => poisoned.into_inner(),
        };
        TerminalSnapshot {
            grid: shared.engine.grid().clone(),
            status: shared.status.clone(),
        }
    }
}

impl Drop for LiveTerminal {
    fn drop(&mut self) {
        // Best effort: ask the child to exit so the reader thread sees EOF.
        let _ = self.pty.terminate();
    }
}

/// Apply one reader event to the shared state. Returns `true` once the reader
/// has finished (EOF or error) and the relay thread should stop.
fn apply_event(shared: &Mutex<Shared>, event: TerminalIoEvent) -> bool {
    let mut shared = match shared.lock() {
        Ok(shared) => shared,
        Err(poisoned) => poisoned.into_inner(),
    };
    match event {
        TerminalIoEvent::Output(bytes) => {
            shared.engine.feed(&bytes);
            false
        }
        TerminalIoEvent::Eof => {
            shared.status = TerminalStatus::Exited;
            true
        }
        TerminalIoEvent::ReadError(error) => {
            shared.status = TerminalStatus::Failed(error);
            true
        }
    }
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows PTY output is not reliable on GitHub Actions"
    )]
    fn spawns_a_shell_echoes_input_and_reports_it_in_the_grid() {
        let command = if cfg!(windows) {
            CommandSpec::new("cmd.exe", Vec::<String>::new())
        } else {
            CommandSpec::new("/bin/sh", Vec::<String>::new())
        };

        static WAKES: AtomicUsize = AtomicUsize::new(0);
        WAKES.store(0, Ordering::SeqCst);
        let wake: WakeFn = Box::new(|| {
            WAKES.fetch_add(1, Ordering::SeqCst);
        });

        let mut terminal = LiveTerminal::spawn(&command, PtyDimensions::new(24, 80), wake).unwrap();
        terminal.send_input(b"printf cockpit-live\\n\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = false;
        while Instant::now() < deadline {
            let snapshot = terminal.snapshot();
            if (0..snapshot.grid.height())
                .filter_map(|row| snapshot.grid.row_text(row))
                .any(|line| line.contains("cockpit-live"))
            {
                seen = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(seen, "expected echoed output in the terminal grid");
        assert!(WAKES.load(Ordering::SeqCst) > 0, "wake was never called");
    }
}
