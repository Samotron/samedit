//! Hotkey errors.

use crate::chord::Hotkey;

/// Errors from chord parsing or registration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    #[error("empty chord")]
    EmptyChord,

    #[error("chord has modifiers but no key: {0:?}")]
    MissingKey(String),

    #[error("unknown modifier token: {0:?}")]
    UnknownToken(String),

    #[error("unknown key: {0:?}")]
    UnknownKey(String),

    /// The chord is already owned — by another app, or already registered here.
    /// Surfaced to the user (a tray toast) at register time; never a silent
    /// failure (plan M12.3).
    #[error("chord already in use: {chord}")]
    Conflict { chord: String, hotkey: Hotkey },
}
