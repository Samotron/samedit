//! Splash painter — v0.6 M6.2.
//!
//! Paints the cockpit logo and a phase-progress indicator while the
//! [`HydrationDriver`](crate::hydration::HydrationDriver) walks through
//! the cold-start phases. Pure render output: takes a
//! [`HydrationProgress`] snapshot and a [`Theme`] and emits draw
//! commands. Lives in the binary because it depends on [`Painter`]; the
//! progress data model is owned by `cockpit-ui` so it stays headless
//! (AGENTS §2 hard rule 1).

use cockpit_render::theme::Color;
use cockpit_render::{Painter, Rect, Theme, Viewport};
use cockpit_ui::HydrationProgress;

use crate::app::{FONT, PAD};

/// Logical row height for the per-phase list in the splash.
const SPLASH_ROW_H: f32 = 18.0;
/// Logical width of the centred progress card.
const SPLASH_CARD_WIDTH: f32 = 420.0;
/// Logical height of the progress bar bar within the card.
const SPLASH_BAR_HEIGHT: f32 = 6.0;

/// Paint the splash for the supplied [`HydrationProgress`]. Idempotent —
/// each call emits a complete frame.
pub fn paint_splash(
    painter: &mut Painter,
    viewport: Viewport,
    progress: &HydrationProgress,
    theme: &Theme,
) {
    let width = viewport.width as f32;
    let height = viewport.height as f32;

    // Solid background — matches the cockpit theme so the splash is
    // visually continuous with the live UI that follows.
    painter.rect(Rect::new(0.0, 0.0, width, height), theme.background);

    let card_w = SPLASH_CARD_WIDTH.min(width - 2.0 * PAD).max(160.0);
    let card_h = SPLASH_ROW_H * 9.0 + PAD * 4.0;
    let card_x = ((width - card_w) / 2.0).max(0.0);
    let card_y = ((height - card_h) / 2.0).max(0.0);

    painter.rect(
        Rect::new(card_x, card_y, card_w, card_h),
        theme.pane_background,
    );

    let mut text_y = card_y + PAD;
    painter.text(
        card_x + PAD,
        text_y,
        "Coding Cockpit".to_string(),
        theme.text,
        FONT + 4.0,
    );
    text_y += SPLASH_ROW_H + 2.0;

    let phase_label = progress.current_label().to_string();
    let phase_color = if progress.is_failed() {
        theme.diagnostic_error
    } else {
        theme.muted_text
    };
    painter.text(card_x + PAD, text_y, phase_label, phase_color, FONT);
    text_y += SPLASH_ROW_H + PAD;

    // Progress bar — track + filled fraction.
    let bar_x = card_x + PAD;
    let bar_w = card_w - 2.0 * PAD;
    let bar_y = text_y;
    painter.rect(
        Rect::new(bar_x, bar_y, bar_w, SPLASH_BAR_HEIGHT),
        theme.pane_border,
    );
    let fill_w = (bar_w * progress.fraction().clamp(0.0, 1.0)).max(0.0);
    if fill_w > 0.0 {
        let fill_color = if progress.is_failed() {
            theme.diagnostic_error
        } else {
            theme.accent
        };
        painter.rect(
            Rect::new(bar_x, bar_y, fill_w, SPLASH_BAR_HEIGHT),
            fill_color,
        );
    }
    text_y += SPLASH_BAR_HEIGHT + PAD;

    // One row per phase — completed phases show their elapsed time,
    // the active phase is highlighted, pending phases are muted.
    paint_phase_rows(
        painter,
        progress,
        theme,
        card_x + PAD,
        text_y,
        card_w - 2.0 * PAD,
    );
}

fn paint_phase_rows(
    painter: &mut Painter,
    progress: &HydrationProgress,
    theme: &Theme,
    x: f32,
    mut y: f32,
    _width: f32,
) {
    use cockpit_ui::HydrationPhase;

    let completed_set: std::collections::HashMap<HydrationPhase, u64> = progress
        .completed()
        .iter()
        .map(|c| (c.phase, c.elapsed_us))
        .collect();
    let current = progress.current();

    for phase in HydrationPhase::ALL {
        let elapsed = completed_set.get(&phase).copied();
        let active = Some(phase) == current;
        let (prefix, color) = if elapsed.is_some() {
            ("[done]", theme.muted_text)
        } else if active {
            ("[..]  ", theme.accent)
        } else {
            ("[ ]   ", text_pending(theme))
        };

        let mut row = format!("{prefix} {}", phase.label());
        if let Some(us) = elapsed {
            row.push_str(&format!("  ({} ms)", us / 1000));
        }
        painter.text(x, y, row, color, FONT);
        y += SPLASH_ROW_H;
    }
}

fn text_pending(theme: &Theme) -> Color {
    // Slightly dimmer than muted so the active row stands out.
    let m = theme.muted_text;
    Color::rgba(m.r * 0.75, m.g * 0.75, m.b * 0.75, m.a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_render::DrawCommand;

    fn viewport() -> Viewport {
        Viewport {
            width: 1280,
            height: 800,
            scale: 1.0,
        }
    }

    #[test]
    fn splash_paints_a_card_and_every_phase_row() {
        let mut painter = Painter::new();
        let progress = HydrationProgress::default_phases();
        paint_splash(&mut painter, viewport(), &progress, &Theme::default());

        let texts: Vec<&str> = painter
            .commands()
            .iter()
            .filter_map(|c| match c {
                DrawCommand::Text(run) => Some(run.text.as_str()),
                _ => None,
            })
            .collect();

        assert!(
            texts.iter().any(|t| t.contains("Coding Cockpit")),
            "title row missing from splash: {texts:?}",
        );
        for phase in cockpit_ui::HydrationPhase::ALL {
            assert!(
                texts.iter().any(|t| t.contains(phase.label())),
                "phase `{}` missing from splash: {texts:?}",
                phase.label(),
            );
        }
    }

    #[test]
    fn failed_progress_renders_the_error_message_in_red() {
        let mut painter = Painter::new();
        let mut progress = HydrationProgress::default_phases();
        progress.begin_next();
        progress.fail("detect: permission denied");
        paint_splash(&mut painter, viewport(), &progress, &Theme::default());

        let theme = Theme::default();
        let has_error_text = painter.commands().iter().any(|c| {
            matches!(c,
                DrawCommand::Text(run)
                    if run.text.contains("permission denied") && run.color == theme.diagnostic_error
            )
        });
        assert!(
            has_error_text,
            "error label must use diagnostic_error colour"
        );
    }
}
