//! `cockpit-terminal` — terminal integration.
//!
//! Houses the PTY wrapper (`portable-pty`), the terminal engine (`termwiz`),
//! the Zellij launcher, the editor↔terminal bridge, and path detection.
//!
//! [`path_detect`] is the first piece implemented (v0.1 milestone M1.7); the
//! rest follows in M1.11 / M1.12.

pub mod engine;
pub mod io_thread;
pub mod live;
pub mod path_detect;
pub mod pty;
pub mod session;
pub mod termwiz_engine;
pub mod zellij;
