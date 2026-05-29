//! IPC error type.

use std::io;

/// Errors from framing, (de)serialisation, or the transport.
#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("frame too large: {len} bytes (max {max})")]
    FrameTooLarge { len: usize, max: usize },

    #[error("connection closed mid-frame")]
    UnexpectedEof,

    #[error("unknown frame encoding tag: {0}")]
    UnknownEncoding(u8),

    #[error("CBOR encode error: {0}")]
    CborEncode(String),

    #[error("CBOR decode error: {0}")]
    CborDecode(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, IpcError>;
