//! `cockpit-quick` — the Raycast-style quick-action launcher sibling binary
//! (v0.13 M13.5).
//!
//! Same shape as `cockpit-jot`: a long-lived tray process that summons a
//! frameless popover on a global hotkey, fuzzy-searches across action
//! providers, and dispatches the chosen action — driving a running cockpit
//! over IPC when needed.
//!
//! This crate is split, exactly like `cockpit-jot`, into:
//!
//! - a tested, backend-free [`QuickController`] (event → intent) plus the
//!   [`loader`] glue that assembles a [`cockpit_launcher::Launcher`] from
//!   `launcher.toml`, and
//! - a thin shell that owns the tray icon, global hotkey, and winit popover.
//!
//! The real `tray-icon` + `global-hotkey` + winit popover loop is the
//! display-bound follow-up, gated behind the `ui-smoke` feature so it lands
//! once a windowing CI leg can smoke-test it. Until then `main.rs` is a
//! headless CLI over the same controller — `search` prints the ranked rows and
//! `run` prints the intent the popover would carry out — usable from scripts
//! and editor keybindings today.

pub mod controller;
pub mod loader;

pub use controller::{QuickController, QuickEvent, QuickIntent};
pub use loader::{ProviderInputs, build_launcher, expand_paths, position_label};
