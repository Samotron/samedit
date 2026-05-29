//! `cockpit-jot` — the Org capture/agenda tray app (v0.12 M12.6).
//!
//! Split into a tested, backend-free brain and a thin glue shell:
//!
//! - [`app::JotController`] turns events (hotkey / tray / popover keys) into
//!   intents (show/dismiss popover, write a file, open in cockpit, quit),
//!   driving the [`cockpit_ui`] org view-models over a live
//!   [`cockpit_org::OrgRoot`]. Fully headless and unit-tested.
//! - The binary's `main.rs` is the platform glue: load the org root from disk,
//!   build the controller, and (behind the `ui-smoke` feature, as a follow-up)
//!   run the `tray-icon` + `global-hotkey` + winit-popover event loop that
//!   feeds events in and executes intents. That layer needs a display to
//!   smoke-test, so it ships when it can be verified.

pub mod app;
pub mod loader;

pub use app::{HotkeyAction, JotController, JotIntent, Surface};
pub use loader::load_root;
