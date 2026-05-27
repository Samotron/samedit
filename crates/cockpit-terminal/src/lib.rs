//! `cockpit-terminal` — terminal integration.
//!
//! Houses the PTY wrapper (`portable-pty`), the terminal engine (`termwiz`),
//! the (deprecated) Zellij launcher, the editor↔terminal bridge, and path
//! detection.
//!
//! [`path_detect`] recognises file references in output (M1.7) and [`bridge`]
//! scans the parsed grid for them so the UI can jump to a file (M2.6).

pub mod bridge;
pub mod command;
pub mod engine;
pub mod io_thread;
pub mod live;
pub mod path_detect;
pub mod pty;
pub mod session;
pub mod termwiz_engine;
pub mod zellij;

pub use command::CommandSpec;
