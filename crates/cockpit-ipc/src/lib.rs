//! `cockpit-ipc` — IPC protocol + transport for Coding Cockpit's sibling
//! binaries (v0.12 M12.1).
//!
//! The cockpit, the jot tray app (v0.12), and the launcher (v0.13) are separate
//! processes (one `winit` event loop per process — AGENTS §1.7). They talk over
//! a single user-only socket, addressing messages with a [`ServiceId`] tag.
//!
//! - **Wire format:** length-prefixed frames; the body is a one-byte
//!   [`Encoding`] tag plus the encoded [`Envelope`]. CBOR is the production
//!   encoding (small, fast, `serde` schema-evolution friendly); JSON is the
//!   newline-debuggable fallback for `socat`. A reader accepts either on the
//!   same socket.
//! - **Transport:** a Unix domain socket (default
//!   [`default_socket_path`] = `$XDG_RUNTIME_DIR/cockpit.sock`). The Windows
//!   named-pipe transport ([`WINDOWS_PIPE_NAME`]) is a follow-up; the wire
//!   types and codec are platform-independent and fully tested today.
//! - **Auth:** none, by design — the socket lives in a user-only directory.
//!   This is not a remote protocol.
//!
//! The transport is generic over the payload, so each service defines its own
//! message enum and the codec carries it verbatim.

pub mod codec;
pub mod error;
pub mod wire;

#[cfg(unix)]
pub mod transport;

pub use codec::{MAX_FRAME, read_message, write_message};
pub use error::{IpcError, Result};
pub use wire::{Encoding, Envelope, ServiceId};

#[cfg(unix)]
pub use transport::{Connection, IpcListener, default_socket_path};

/// The Windows named-pipe address the named-pipe transport will use.
///
/// Exposed unconditionally for documentation and cross-platform path logic; the
/// pipe transport itself is not implemented yet (Unix socket ships first).
pub const WINDOWS_PIPE_NAME: &str = r"\\.\pipe\cockpit";
