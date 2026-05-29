//! `cockpit-paint` — headless presentation primitives (v0.12 M12.4).
//!
//! The immediate-mode [`Painter`], the [`Theme`]/[`Color`] palette, and the
//! [`PopoverContent`] trait that frameless popover shells host. None of this
//! touches `winit`, `glow`, or any GPU/window crate, so it is shared by
//! [`cockpit-render`](../cockpit_render/index.html) (the main window) and the
//! sibling popover binaries (`cockpit-jot`, the v0.13 launcher) without
//! dragging the renderer into a tray app.
//!
//! Before M12.4 the painter and theme lived in `cockpit-render`; extracting
//! them here lets a popover paint with the same primitives the main window
//! uses — and reuse the on-disk glyph atlas — while keeping AGENTS §1's
//! "only `cockpit-render` depends on `winit`/`glow`" rule intact (the rule
//! reads per-binary: each event loop owns its own window shell).

pub mod painter;
pub mod popover;
pub mod theme;

pub use painter::{DrawCommand, Painter, Rect, RectBatch, TextRun};
pub use popover::{PopoverContent, PopoverViewport};
pub use theme::{Color, SyntaxTheme, Theme};
