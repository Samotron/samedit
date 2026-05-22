//! UI-facing terminal session state.
//!
//! This layer connects reader-thread output events to a [`TerminalEngine`].
//! It is deliberately independent from `portable-pty` so tests can drive the
//! same behavior with in-memory events.

use crate::{
    engine::{ScreenGrid, TerminalEngine},
    io_thread::TerminalIoEvent,
};

/// Terminal lifecycle state visible to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStatus {
    Running,
    Exited,
    Failed(String),
}

/// A terminal engine plus lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSession<E> {
    engine: E,
    status: TerminalStatus,
    output_events: usize,
}

impl<E: TerminalEngine> TerminalSession<E> {
    /// Create a running session around an engine.
    pub fn new(engine: E) -> Self {
        Self {
            engine,
            status: TerminalStatus::Running,
            output_events: 0,
        }
    }

    /// Apply one reader-thread event.
    pub fn handle_io_event(&mut self, event: TerminalIoEvent) {
        match event {
            TerminalIoEvent::Output(bytes) => {
                self.engine.feed(&bytes);
                self.output_events += 1;
            }
            TerminalIoEvent::Eof => {
                self.status = TerminalStatus::Exited;
            }
            TerminalIoEvent::ReadError(error) => {
                self.status = TerminalStatus::Failed(error);
            }
        }
    }

    /// Resize the engine grid.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.engine.resize(width, height);
    }

    /// Current grid.
    pub fn grid(&self) -> &ScreenGrid {
        self.engine.grid()
    }

    /// Current lifecycle state.
    pub fn status(&self) -> &TerminalStatus {
        &self.status
    }

    /// Number of output events applied.
    pub fn output_events(&self) -> usize {
        self.output_events
    }

    /// Borrow the underlying engine.
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// Borrow the underlying engine mutably.
    pub fn engine_mut(&mut self) -> &mut E {
        &mut self.engine
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::{Cursor, GridEngine};

    use super::*;

    #[test]
    fn output_events_feed_the_engine_grid() {
        let mut session = TerminalSession::new(GridEngine::new(8, 2));

        session.handle_io_event(TerminalIoEvent::Output(b"hello".to_vec()));

        assert_eq!(session.grid().row_text(0).unwrap(), "hello   ");
        assert_eq!(session.grid().cursor(), Cursor { row: 0, col: 5 });
        assert_eq!(session.output_events(), 1);
        assert_eq!(session.status(), &TerminalStatus::Running);
    }

    #[test]
    fn eof_marks_session_exited() {
        let mut session = TerminalSession::new(GridEngine::new(8, 2));

        session.handle_io_event(TerminalIoEvent::Eof);

        assert_eq!(session.status(), &TerminalStatus::Exited);
    }

    #[test]
    fn read_errors_mark_session_failed() {
        let mut session = TerminalSession::new(GridEngine::new(8, 2));

        session.handle_io_event(TerminalIoEvent::ReadError("boom".to_string()));

        assert_eq!(
            session.status(),
            &TerminalStatus::Failed("boom".to_string())
        );
    }

    #[test]
    fn resize_updates_engine_grid() {
        let mut session = TerminalSession::new(GridEngine::new(8, 2));
        session.handle_io_event(TerminalIoEvent::Output(b"abcdef".to_vec()));

        session.resize(4, 3);

        assert_eq!(session.grid().width(), 4);
        assert_eq!(session.grid().height(), 3);
        assert_eq!(session.grid().row_text(0).unwrap(), "abcd");
    }
}
