//! `cockpit-lsp` — the LSP client foundation (spec §19 / M3.5).
//!
//! Three layered pieces, each headless-testable on its own:
//!
//! * [`codec`] — `Content-Length`-framed reads and writes over any
//!   [`std::io::BufRead`] / [`std::io::Write`].
//! * [`jsonrpc`] — typed JSON-RPC envelopes (request, notification, response).
//! * [`registry`] — `Language` → [`ServerConfig`] map. rust-analyzer is the
//!   only entry for v0.3; the table is the seam where new languages plug in.
//!
//! [`client::LspClient`] composes these into a process-managed client: it
//! spawns the server, runs reader and writer threads (OS threads + channels,
//! never an async runtime — per AGENTS §2 rule #4), and queues outbound
//! requests/notifications so callers never block on the protocol.
//!
//! Per spec §19 the client must never start on app launch, never block the UI,
//! never spawn auto-installed servers, and never run for huge files. Those
//! invariants are policy enforced at the call site (see `cockpit`'s wiring);
//! this crate just provides the transport.

pub mod client;
pub mod codec;
pub mod jsonrpc;
pub mod registry;

pub use client::{ClientError, LspClient, RecvMessage};
pub use codec::CodecError;
pub use jsonrpc::{Notification, Request, Response, RpcError};
pub use registry::ServerConfig;
