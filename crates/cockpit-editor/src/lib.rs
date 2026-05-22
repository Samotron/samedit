//! `cockpit-editor` — the editor core.
//!
//! A `ropey`-backed [`Buffer`] (spec §15), a [`Cursor`], a reversible-edit
//! [`History`] for undo/redo, and substring [`search`]. Fully headless: no
//! UI, no rendering, and no I/O beyond what the caller passes in.
//!
//! The Vim state machine (milestone M1.2) is built on top of these pieces.

pub mod buffer;
pub mod cursor;
pub mod editor;
pub mod search;
pub mod undo;
pub mod vim;

pub use buffer::Buffer;
pub use cursor::Cursor;
pub use editor::{Editor, EditorSignal};
pub use undo::History;
