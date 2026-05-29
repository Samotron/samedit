//! Wire types: the service routing tag, the message envelope, and the two
//! payload encodings.

use serde::{Deserialize, Serialize};

/// Which service a message is addressed to. A single socket multiplexes all
/// three; the envelope's `service` field is the routing tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceId {
    /// The Org jot service (v0.12).
    Jot,
    /// The launcher service (v0.13).
    Launcher,
    /// The main cockpit.
    Cockpit,
}

impl ServiceId {
    /// Stable lowercase name, also used by the JSON debug fallback.
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceId::Jot => "jot",
            ServiceId::Launcher => "launcher",
            ServiceId::Cockpit => "cockpit",
        }
    }
}

/// A routed message: a service tag plus a typed payload. The transport is
/// generic over the payload, so each service defines its own message enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub service: ServiceId,
    pub payload: T,
}

impl<T> Envelope<T> {
    pub fn new(service: ServiceId, payload: T) -> Self {
        Envelope { service, payload }
    }
}

/// Payload encoding. CBOR is the production wire format (small, fast,
/// schema-evolution friendly); JSON is the newline-debuggable fallback for
/// poking at the socket with `socat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Cbor,
    Json,
}

impl Encoding {
    /// The one-byte tag that prefixes each frame's payload, letting a reader
    /// accept either encoding on the same socket.
    pub(crate) fn tag(self) -> u8 {
        match self {
            Encoding::Cbor => 0,
            Encoding::Json => 1,
        }
    }

    pub(crate) fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Encoding::Cbor),
            1 => Some(Encoding::Json),
            _ => None,
        }
    }
}
