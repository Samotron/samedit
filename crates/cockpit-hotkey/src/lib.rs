//! `cockpit-hotkey` — global-hotkey chord grammar, conflict detection, and a
//! headless hotkey bus (v0.12 M12.3).
//!
//! The jot tray app (and the v0.13 launcher) bind global chords like
//! `Ctrl+Alt+A`. This crate owns the *backend-free* half of that:
//!
//! - [`Hotkey::parse`] turns a chord string into a typed, normalised
//!   [`Hotkey`] (case-insensitive, modifier aliases folded).
//! - [`HotkeyBus`] is the registration seam; registration is fallible and a
//!   chord already owned by another app returns [`HotkeyError::Conflict`] —
//!   surfaced to the user, never a silent failure (plan M12.3).
//! - [`FakeHotkeyBus`] is the in-memory test double: it models external
//!   conflicts and simulated key presses.
//!
//! The real `global-hotkey` backend (which actually registers with the OS) is a
//! follow-up behind the `ui-smoke` feature — it needs a windowing CI leg to
//! smoke-test, so it ships when it can be verified. Everything here is pure and
//! covered by the fast test leg.

pub mod bus;
pub mod chord;
pub mod error;

pub use bus::{FakeHotkeyBus, HotkeyBus, HotkeyId};
pub use chord::{Hotkey, Key, Modifiers, NamedKey};
pub use error::HotkeyError;
