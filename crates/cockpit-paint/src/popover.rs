//! Popover content contract (v0.12 M12.4).
//!
//! A frameless popover shell (the display-bound `winit`+`glow` window that
//! `cockpit-jot` and the v0.13 launcher each own) hosts a single
//! [`PopoverContent`]: a headless view-model that turns key chords into state
//! and paints itself with the shared [`Painter`](crate::painter::Painter). The
//! shell owns the window, the always-on-top/centred placement, and the focus
//! tracking; the content owns *what is shown* and *when to dismiss*.
//!
//! Splitting it this way keeps every popover brain unit-testable with no
//! window: the jot capture/agenda popover and the launcher list are both
//! plain `PopoverContent` impls exercised by `#[test]`s, and only the thin
//! shell needs a display server.

use cockpit_commands::KeyChord;

use crate::painter::Painter;
use crate::theme::Theme;

/// Viewport handed to [`PopoverContent::paint`] each frame.
///
/// Physical pixels plus the display scale factor — the same shape as
/// `cockpit_render::Viewport`, duplicated here so popover content stays free
/// of any `winit` dependency. The shell translates its window size into this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopoverViewport {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// Display scale factor (HiDPI). `1.0` on a standard-density display.
    pub scale: f32,
}

impl PopoverViewport {
    /// Construct a viewport.
    pub fn new(width: u32, height: u32, scale: f32) -> Self {
        Self {
            width,
            height,
            scale,
        }
    }

    /// Logical width (physical width divided by the scale factor, clamped so a
    /// degenerate `scale` can't divide by zero).
    pub fn logical_width(self) -> f32 {
        self.width as f32 / self.scale.max(0.5)
    }

    /// Logical height (physical height divided by the scale factor).
    pub fn logical_height(self) -> f32 {
        self.height as f32 / self.scale.max(0.5)
    }
}

/// The view-model a popover shell hosts.
///
/// Implementors hold headless state (e.g. the org capture/agenda view-models)
/// and turn it into draw commands. The shell never inspects content state
/// beyond this trait — it paints [`paint`](PopoverContent::paint), forwards
/// input to [`on_key`](PopoverContent::on_key) /
/// [`on_text`](PopoverContent::on_text), advances
/// [`tick`](PopoverContent::tick) once per frame, and dismisses when
/// [`wants_exit`](PopoverContent::wants_exit) flips (or per the ESC/focus
/// policy in [`esc_should_dismiss`]).
pub trait PopoverContent {
    /// Theme used to clear the popover background and resolve colours.
    fn theme(&self) -> &Theme;

    /// Advance any per-frame state (cursor blink, animations). Default no-op —
    /// most popover content is purely input-driven.
    fn tick(&mut self) {}

    /// Paint the current state into `painter` for a frame of `viewport`.
    fn paint(&mut self, painter: &mut Painter, viewport: PopoverViewport);

    /// Handle one resolved key chord (key-down only).
    ///
    /// Returns `true` when the content consumed the chord. The shell uses the
    /// return value to decide whether its default behaviour applies — in
    /// particular, an unconsumed `Escape` dismisses the popover (see
    /// [`esc_should_dismiss`]), but content that handles `Escape` itself
    /// (cancelling a sub-mode) returns `true` to keep the popover open.
    fn on_key(&mut self, chord: KeyChord) -> bool;

    /// Handle committed text input (insert-mode typing). Default: ignored.
    fn on_text(&mut self, _text: &str) {}

    /// True once the content wants the popover dismissed — e.g. a capture
    /// committed, or the user chose an item. The shell polls this after every
    /// `on_key`/`on_text` and tears the window down when it flips.
    fn wants_exit(&self) -> bool;
}

/// Whether an unconsumed key chord should dismiss the popover.
///
/// Pure dismissal policy the (display-bound) shell calls so the ESC rule stays
/// unit-tested: a bare `Escape` that the content did **not** consume dismisses
/// the popover. `consumed` is the value returned by
/// [`PopoverContent::on_key`]. Focus-loss dismissal is the shell's concern
/// (it has no chord) and is configurable per the M12.4 plan; this helper only
/// covers the keyboard path.
pub fn esc_should_dismiss(consumed: bool, chord: &KeyChord) -> bool {
    !consumed && is_bare_escape(chord)
}

/// True when `chord` is a single, modifier-free `Escape` stroke.
fn is_bare_escape(chord: &KeyChord) -> bool {
    let strokes = chord.strokes();
    match strokes.first() {
        Some(stroke) if strokes.len() == 1 => {
            stroke.key() == "Escape" && stroke.modifiers().is_none()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::painter::{DrawCommand, Rect};

    fn chord(input: &str) -> KeyChord {
        input.parse().expect("chord parses")
    }

    /// Minimal reference content: a single-field text editor that appends
    /// typed characters, treats `Enter` as commit (sets `wants_exit`), and
    /// intercepts `Escape` only while it holds unsaved text (so an empty
    /// editor lets the shell dismiss on `Escape`, a dirty one cancels its own
    /// edit first). Exercises every trait method.
    #[derive(Default)]
    struct TextPopover {
        theme: Theme,
        text: String,
        committed: bool,
        ticks: u32,
    }

    impl PopoverContent for TextPopover {
        fn theme(&self) -> &Theme {
            &self.theme
        }

        fn tick(&mut self) {
            self.ticks += 1;
        }

        fn paint(&mut self, painter: &mut Painter, viewport: PopoverViewport) {
            painter.rect(
                Rect::new(0.0, 0.0, viewport.width as f32, viewport.height as f32),
                self.theme.background,
            );
            if !self.text.is_empty() {
                painter.text(8.0, 8.0, self.text.clone(), self.theme.text, 14.0);
            }
        }

        fn on_key(&mut self, chord: KeyChord) -> bool {
            let stroke = chord.strokes().first().cloned();
            let Some(stroke) = stroke else {
                return false;
            };
            match stroke.key() {
                "Enter" if stroke.modifiers().is_none() => {
                    self.committed = true;
                    true
                }
                "Escape" if stroke.modifiers().is_none() && !self.text.is_empty() => {
                    // Dirty editor cancels its own edit instead of dismissing.
                    self.text.clear();
                    true
                }
                "Backspace" if stroke.modifiers().is_none() => {
                    self.text.pop();
                    true
                }
                _ => false,
            }
        }

        fn on_text(&mut self, text: &str) {
            self.text.push_str(text);
        }

        fn wants_exit(&self) -> bool {
            self.committed
        }
    }

    #[test]
    fn tick_advances_content_state() {
        let mut popover = TextPopover::default();
        popover.tick();
        popover.tick();
        assert_eq!(popover.ticks, 2);
    }

    #[test]
    fn paint_clears_background_and_renders_text() {
        let mut popover = TextPopover::default();
        popover.on_text("hi");
        let mut painter = Painter::new();
        popover.paint(&mut painter, PopoverViewport::new(200, 100, 1.0));
        // Background rect + the text run.
        assert_eq!(painter.commands().len(), 2);
        assert!(matches!(painter.commands()[0], DrawCommand::Rect { .. }));
        assert!(matches!(painter.commands()[1], DrawCommand::Text(_)));
    }

    #[test]
    fn enter_commits_and_requests_exit() {
        let mut popover = TextPopover::default();
        popover.on_text("note");
        assert!(!popover.wants_exit());
        let consumed = popover.on_key(chord("Enter"));
        assert!(consumed);
        assert!(popover.wants_exit());
    }

    #[test]
    fn bare_escape_on_empty_content_dismisses() {
        let mut popover = TextPopover::default();
        let consumed = popover.on_key(chord("Escape"));
        assert!(!consumed, "empty editor leaves Escape for the shell");
        assert!(esc_should_dismiss(consumed, &chord("Escape")));
    }

    #[test]
    fn escape_consumed_by_dirty_content_does_not_dismiss() {
        let mut popover = TextPopover::default();
        popover.on_text("draft");
        let consumed = popover.on_key(chord("Escape"));
        assert!(consumed, "dirty editor cancels its own edit");
        assert!(!esc_should_dismiss(consumed, &chord("Escape")));
        assert_eq!(popover.text, "");
    }

    #[test]
    fn non_escape_keys_never_trigger_dismissal() {
        // Even unconsumed, only Escape drives the shell's default dismissal.
        assert!(!esc_should_dismiss(false, &chord("Enter")));
        assert!(!esc_should_dismiss(false, &chord("Ctrl+c")));
        // A modified Escape is not the bare Escape the policy matches.
        assert!(!esc_should_dismiss(false, &chord("Ctrl+Escape")));
    }
}
