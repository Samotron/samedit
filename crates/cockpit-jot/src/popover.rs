//! The jot popover content (v0.12 M12.4 / M12.6).
//!
//! [`JotPopover`] is the [`PopoverContent`] the (display-bound) `winit` popover
//! shell hosts. It wraps the headless [`JotController`] and is itself fully
//! headless: it turns the popover's painter + key/text events into controller
//! calls, paints whichever [`Surface`] is live, and exposes the resulting
//! [`JotIntent`]s for the shell to carry out. The shell owns the window, the
//! tray icon, and the global hotkey; this layer owns *what the popover shows*
//! and *how its keys map onto the controller* — all unit-tested without a
//! display.
//!
//! Input model mirrors `cockpit-render`'s shell: every key press arrives as a
//! [`KeyChord`] via [`on_key`](PopoverContent::on_key), and a *plain* printable
//! key additionally arrives as committed text via
//! [`on_text`](PopoverContent::on_text). Free-text fields (the capture editor
//! buffer, the agenda `/` filter) consume `on_text`; navigation and commands
//! consume `on_key`. Each surface ignores the channel it doesn't use so a key
//! is never handled twice.

use cockpit_commands::KeyChord;
use cockpit_paint::{Painter, PopoverContent, PopoverViewport, Rect, Theme};
use cockpit_ui::{AgendaRowKind, AgendaView, CapturePhase, CaptureView, OrgListView};

use crate::app::{HotkeyAction, JotController, JotIntent, Surface};
use cockpit_tray::TrayEvent;

/// Logical-pixel layout constants for the popover. Multiplied by the display
/// scale at paint time.
const PAD: f32 = 16.0;
const ROW_H: f32 = 22.0;
const FONT: f32 = 15.0;
const TITLE_FONT: f32 = 20.0;

/// Coarse classification of the live surface, computed up front so key routing
/// doesn't hold a borrow of the controller across a mutating call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceKind {
    Hidden,
    CapturePicking,
    CaptureEditing,
    Agenda,
    Overview,
}

/// The jot popover: the controller plus the popover-local UI state (the `/`
/// filter mode) and a queue of intents for the shell to drain.
pub struct JotPopover {
    controller: JotController,
    theme: Theme,
    pending: Vec<JotIntent>,
    /// `true` while the agenda's `/` filter input is active — keystrokes edit
    /// the query instead of navigating.
    filtering: bool,
    /// One-shot guard: the `/` that opens the filter is also delivered as
    /// committed text right after `on_key`; swallow that one `on_text` so it
    /// doesn't land in the query.
    swallow_next_text: bool,
}

impl JotPopover {
    /// Wrap a controller with the default theme.
    pub fn new(controller: JotController) -> Self {
        Self::with_theme(controller, Theme::default())
    }

    /// Wrap a controller with an explicit theme.
    pub fn with_theme(controller: JotController, theme: Theme) -> Self {
        Self {
            controller,
            theme,
            pending: Vec::new(),
            filtering: false,
            swallow_next_text: false,
        }
    }

    /// The wrapped controller (read-only — for the shell's IPC/tray wiring and
    /// tests).
    pub fn controller(&self) -> &JotController {
        &self.controller
    }

    /// `true` while the agenda `/` filter input is active.
    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    /// Drain the intents produced since the last call. The shell calls this
    /// after feeding any event and carries the intents out (show/dismiss the
    /// window, write a file, open in cockpit, quit).
    pub fn take_intents(&mut self) -> Vec<JotIntent> {
        std::mem::take(&mut self.pending)
    }

    /// Feed a global hotkey through the controller, resetting popover-local UI
    /// state for the freshly opened surface.
    pub fn on_hotkey(&mut self, action: HotkeyAction) {
        self.reset_ui();
        let intents = self.controller.on_hotkey(action);
        self.pending.extend(intents);
    }

    /// Feed a tray event through the controller.
    pub fn on_tray(&mut self, event: TrayEvent) {
        self.reset_ui();
        let intents = self.controller.on_tray(event);
        self.pending.extend(intents);
    }

    /// Dismiss the popover (ESC / focus loss) through the controller.
    pub fn dismiss(&mut self) {
        self.reset_ui();
        let intents = self.controller.dismiss();
        self.pending.extend(intents);
    }

    fn reset_ui(&mut self) {
        self.filtering = false;
        self.swallow_next_text = false;
    }

    fn surface_kind(&self) -> SurfaceKind {
        match self.controller.surface() {
            Surface::Hidden => SurfaceKind::Hidden,
            Surface::Capture(v) if v.phase() == CapturePhase::Editing => {
                SurfaceKind::CaptureEditing
            }
            Surface::Capture(_) => SurfaceKind::CapturePicking,
            Surface::Agenda(_) => SurfaceKind::Agenda,
            Surface::Overview(_) => SurfaceKind::Overview,
        }
    }

    /// Current agenda filter query, cloned (empty off the agenda surface).
    fn agenda_query(&self) -> String {
        match self.controller.surface() {
            Surface::Agenda(v) => v.filter_query().to_string(),
            _ => String::new(),
        }
    }
}

impl PopoverContent for JotPopover {
    fn theme(&self) -> &Theme {
        &self.theme
    }

    fn paint(&mut self, painter: &mut Painter, viewport: PopoverViewport) {
        let scale = viewport.scale.max(0.5);
        let w = viewport.width as f32;
        let h = viewport.height as f32;

        // Background + accent top bar.
        painter.rect(Rect::new(0.0, 0.0, w, h), self.theme.pane_background);
        painter.rect(Rect::new(0.0, 0.0, w, 2.0 * scale), self.theme.accent);

        match self.controller.surface() {
            Surface::Hidden => {}
            Surface::Capture(view) => paint_capture(painter, &self.theme, view, scale),
            Surface::Agenda(view) => {
                paint_agenda(painter, &self.theme, view, self.filtering, scale)
            }
            Surface::Overview(view) => paint_overview(painter, &self.theme, view, scale),
        }
    }

    fn on_key(&mut self, chord: KeyChord) -> bool {
        let Some(stroke) = chord.strokes().first() else {
            return false;
        };
        let key = stroke.key();
        let mods = stroke.modifiers();
        let plain = mods.is_none();
        let kind = self.surface_kind();

        // Escape: leave the filter sub-mode if active, else dismiss the popover.
        if key == "Escape" && plain {
            if kind == SurfaceKind::Agenda && self.filtering {
                self.filtering = false;
                self.controller.agenda_set_filter("");
            } else {
                self.dismiss();
            }
            return true;
        }

        match kind {
            SurfaceKind::Hidden => false,
            // Template selection happens via `on_text`; the picker ignores
            // navigation keys.
            SurfaceKind::CapturePicking => false,
            SurfaceKind::CaptureEditing => match key {
                "Enter" if mods.ctrl => {
                    let intents = self.controller.capture_commit();
                    self.pending.extend(intents);
                    true
                }
                "Enter" if plain => {
                    self.controller.capture_insert_str("\n");
                    true
                }
                "Backspace" if plain => {
                    self.controller.capture_backspace();
                    true
                }
                "ArrowLeft" if plain => {
                    self.controller.capture_move_left();
                    true
                }
                "ArrowRight" if plain => {
                    self.controller.capture_move_right();
                    true
                }
                // Printable input flows through `on_text`.
                _ => false,
            },
            SurfaceKind::Agenda if self.filtering => match key {
                "Enter" if plain => {
                    // Commit the filter, leave the input mode (query is kept).
                    self.filtering = false;
                    true
                }
                "Backspace" if plain => {
                    let mut q = self.agenda_query();
                    q.pop();
                    self.controller.agenda_set_filter(q);
                    true
                }
                _ => false,
            },
            SurfaceKind::Agenda => match key {
                "ArrowDown" | "j" if plain => {
                    self.controller.cursor_down();
                    true
                }
                "ArrowUp" | "k" if plain => {
                    self.controller.cursor_up();
                    true
                }
                "Tab" if plain => {
                    self.controller.agenda_cycle_mode();
                    true
                }
                "Enter" if plain => {
                    let intents = self.controller.activate_selection();
                    self.pending.extend(intents);
                    true
                }
                "/" if plain => {
                    self.filtering = true;
                    self.controller.agenda_set_filter("");
                    // The `/` is echoed as committed text next — drop it.
                    self.swallow_next_text = true;
                    true
                }
                _ => false,
            },
            SurfaceKind::Overview => match key {
                "ArrowDown" | "j" if plain => {
                    self.controller.cursor_down();
                    true
                }
                "ArrowUp" | "k" if plain => {
                    self.controller.cursor_up();
                    true
                }
                "Enter" if plain => {
                    let intents = self.controller.activate_selection();
                    self.pending.extend(intents);
                    true
                }
                _ => false,
            },
        }
    }

    fn on_text(&mut self, text: &str) {
        if self.swallow_next_text {
            self.swallow_next_text = false;
            return;
        }
        match self.surface_kind() {
            // The typed key selects a template.
            SurfaceKind::CapturePicking => {
                self.controller.capture_pick(text);
            }
            SurfaceKind::CaptureEditing => {
                self.controller.capture_insert_str(text);
            }
            SurfaceKind::Agenda if self.filtering => {
                let q = format!("{}{}", self.agenda_query(), text);
                self.controller.agenda_set_filter(q);
            }
            _ => {}
        }
    }

    fn wants_exit(&self) -> bool {
        matches!(self.controller.surface(), Surface::Hidden)
    }
}

// ---- painters ---------------------------------------------------------------

/// Draw a title at the top of the content area; returns the `y` for the first
/// content row.
fn paint_title(painter: &mut Painter, theme: &Theme, title: &str, scale: f32) -> f32 {
    painter.text(
        PAD * scale,
        PAD * scale,
        title,
        theme.text,
        TITLE_FONT * scale,
    );
    PAD + TITLE_FONT + 8.0
}

fn paint_capture(painter: &mut Painter, theme: &Theme, view: &CaptureView, scale: f32) {
    match view.phase() {
        CapturePhase::Picking => {
            let mut y = paint_title(painter, theme, "Capture", scale);
            for row in view.template_rows() {
                painter.text(
                    PAD * scale,
                    y * scale,
                    format!("{}   {}", row.key, row.name),
                    theme.text,
                    FONT * scale,
                );
                y += ROW_H;
            }
        }
        CapturePhase::Editing => {
            let mut y = paint_title(painter, theme, "Capture · edit", scale);
            for line in view.buffer().split('\n') {
                painter.text(PAD * scale, y * scale, line, theme.text, FONT * scale);
                y += ROW_H;
            }
            painter.text(
                PAD * scale,
                (y + 8.0) * scale,
                "Ctrl+Enter save · Esc cancel",
                theme.muted_text,
                (FONT - 2.0) * scale,
            );
        }
    }
}

fn paint_agenda(
    painter: &mut Painter,
    theme: &Theme,
    view: &AgendaView,
    filtering: bool,
    scale: f32,
) {
    let mut title = view.mode().label().to_string();
    if filtering || !view.filter_query().is_empty() {
        title = format!("{title}   /{}", view.filter_query());
    }
    let mut y = paint_title(painter, theme, &title, scale);

    let mut item_idx = 0usize;
    for row in view.rows() {
        let is_item = matches!(row.kind, AgendaRowKind::Item);
        let selected = is_item && item_idx == view.cursor();
        if selected {
            painter.rect(
                Rect::new(
                    (PAD - 4.0) * scale,
                    (y - 2.0) * scale,
                    300.0 * scale,
                    ROW_H * scale,
                ),
                theme.selection,
            );
        }
        let color = match row.kind {
            AgendaRowKind::DayHeader | AgendaRowKind::FileHeader => theme.muted_text,
            AgendaRowKind::Item if row.overdue => theme.diagnostic_error,
            AgendaRowKind::Item => theme.text,
        };
        let indent = if is_item { PAD + 12.0 } else { PAD };
        painter.text(indent * scale, y * scale, &row.label, color, FONT * scale);
        if is_item {
            item_idx += 1;
        }
        y += ROW_H;
    }
}

fn paint_overview(painter: &mut Painter, theme: &Theme, view: &OrgListView, scale: f32) {
    let mut y = paint_title(painter, theme, "Org", scale);
    let selected = view.selected_index();
    for (i, row) in view.rows().iter().enumerate() {
        if Some(i) == selected {
            painter.rect(
                Rect::new(
                    (PAD - 4.0) * scale,
                    (y - 2.0) * scale,
                    300.0 * scale,
                    ROW_H * scale,
                ),
                theme.selection,
            );
        }
        let color = if row.is_file_header {
            theme.muted_text
        } else {
            theme.text
        };
        let indent = PAD + (row.level.saturating_sub(1) as f32) * 12.0;
        let label = match &row.todo_keyword {
            Some(kw) => format!("{kw} {}", row.label),
            None => row.label.clone(),
        };
        painter.text(indent * scale, y * scale, label, color, FONT * scale);
        y += ROW_H;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_org::{
        CaptureTarget, CaptureTemplate, NowStamp, OrgConfig, OrgDate, OrgRoot, OrgTime,
    };
    use cockpit_paint::DrawCommand;

    fn now() -> NowStamp {
        NowStamp::new(OrgDate::new(2026, 5, 29), OrgTime::new(9, 0), "Fri")
    }

    fn config() -> OrgConfig {
        OrgConfig {
            root: Some("/org".into()),
            default_todo_keywords: vec!["TODO".into(), "DONE".into()],
            capture: vec![CaptureTemplate {
                key: "t".into(),
                name: "Todo".into(),
                target: CaptureTarget {
                    file: "inbox.org".into(),
                    under: Some("Tasks".into()),
                    datetree: false,
                },
                template: "* TODO %?".into(),
            }],
        }
    }

    fn popover() -> JotPopover {
        let root = OrgRoot::from_files(
            "/org",
            [
                ("/org/inbox.org", "* Tasks\n"),
                (
                    "/org/tasks.org",
                    "* TODO ship it :work:\nSCHEDULED: <2026-05-29 Fri>\n* TODO write docs\nSCHEDULED: <2026-05-29 Fri>\n",
                ),
            ],
        );
        JotPopover::new(JotController::new(config(), root, now()))
    }

    fn chord(s: &str) -> KeyChord {
        s.parse().expect("chord parses")
    }

    fn vp() -> PopoverViewport {
        PopoverViewport::new(420, 320, 1.0)
    }

    #[test]
    fn hidden_until_a_surface_opens() {
        let mut p = popover();
        assert!(p.wants_exit(), "hidden surface wants the window closed");
        p.on_hotkey(HotkeyAction::Agenda);
        assert!(!p.wants_exit());
        // Opening a surface emits ShowPopover for the shell.
        assert_eq!(p.take_intents(), vec![JotIntent::ShowPopover]);
    }

    #[test]
    fn capture_pick_then_type_then_commit() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Capture);
        let _ = p.take_intents();

        // Picker key arrives as committed text and selects the template.
        p.on_text("t");
        assert_eq!(p.surface_kind(), SurfaceKind::CaptureEditing);

        // Typing lands in the %? slot — the picker key never leaked.
        p.on_text("buy milk");
        if let Surface::Capture(v) = p.controller().surface() {
            assert_eq!(v.buffer(), "* TODO buy milk");
        } else {
            panic!("expected capture surface");
        }

        let consumed = p.on_key(chord("Ctrl+Enter"));
        assert!(consumed);
        let intents = p.take_intents();
        assert!(matches!(
            intents.first(),
            Some(JotIntent::WriteFile { source, .. }) if source == "* Tasks\n** TODO buy milk\n"
        ));
        assert_eq!(intents.last(), Some(&JotIntent::DismissPopover));
        assert!(p.wants_exit());
    }

    #[test]
    fn capture_editing_keys_edit_the_buffer() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Capture);
        p.on_text("t");
        p.on_text("abc");
        // Backspace removes the last char.
        assert!(p.on_key(chord("Backspace")));
        if let Surface::Capture(v) = p.controller().surface() {
            assert_eq!(v.buffer(), "* TODO ab");
        }
        // Plain printable keys are not consumed by on_key (on_text handles them).
        assert!(!p.on_key(chord("x")));
    }

    #[test]
    fn agenda_navigates_and_jumps() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        let _ = p.take_intents();

        assert!(p.on_key(chord("ArrowDown")));
        assert!(p.on_key(chord("Enter")));
        let intents = p.take_intents();
        // Second scheduled item is "write docs" on line 2.
        assert!(matches!(
            intents.first(),
            Some(JotIntent::OpenInCockpit { line: 2, .. })
        ));
        assert_eq!(intents.last(), Some(&JotIntent::DismissPopover));
        assert!(p.wants_exit());
    }

    #[test]
    fn agenda_tab_cycles_mode() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        if let Surface::Agenda(v) = p.controller().surface() {
            assert_eq!(v.mode(), cockpit_ui::AgendaMode::Today);
        }
        assert!(p.on_key(chord("Tab")));
        if let Surface::Agenda(v) = p.controller().surface() {
            assert_eq!(v.mode(), cockpit_ui::AgendaMode::Next7Days);
        } else {
            panic!("expected agenda surface");
        }
    }

    #[test]
    fn agenda_filter_swallows_activating_slash_and_filters() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);

        // `/` enters filter mode and is consumed.
        assert!(p.on_key(chord("/")));
        assert!(p.is_filtering());
        // The shell echoes the `/` as committed text; it must be swallowed.
        p.on_text("/");
        assert_eq!(p.agenda_query(), "");

        // Typing builds the query and re-filters the agenda.
        p.on_text("work");
        assert_eq!(p.agenda_query(), "work");
        if let Surface::Agenda(v) = p.controller().surface() {
            // Only the :work:-tagged item survives the filter.
            let items = v
                .rows()
                .iter()
                .filter(|r| matches!(r.kind, AgendaRowKind::Item))
                .count();
            assert_eq!(items, 1);
        }

        // Enter leaves the filter input but keeps the query.
        assert!(p.on_key(chord("Enter")));
        assert!(!p.is_filtering());
        assert_eq!(p.agenda_query(), "work");
    }

    #[test]
    fn agenda_filter_escape_clears_query_and_stays_open() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        let _ = p.take_intents();
        p.on_key(chord("/"));
        p.on_text("/"); // swallowed
        p.on_text("zzz");
        assert_eq!(p.agenda_query(), "zzz");

        assert!(p.on_key(chord("Escape")));
        assert!(!p.is_filtering());
        assert_eq!(p.agenda_query(), "");
        // Escape out of filter mode does NOT dismiss the popover.
        assert!(!p.wants_exit());
        assert!(p.take_intents().is_empty());
    }

    #[test]
    fn escape_dismisses_the_popover() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        let _ = p.take_intents();
        assert!(p.on_key(chord("Escape")));
        assert_eq!(p.take_intents(), vec![JotIntent::DismissPopover]);
        assert!(p.wants_exit());
    }

    #[test]
    fn overview_navigates_and_jumps() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Overview);
        let _ = p.take_intents();
        assert!(p.on_key(chord("ArrowDown")));
        assert!(p.on_key(chord("Enter")));
        let intents = p.take_intents();
        assert!(matches!(
            intents.first(),
            Some(JotIntent::OpenInCockpit { .. })
        ));
        assert!(p.wants_exit());
    }

    #[test]
    fn paint_agenda_emits_background_selection_and_text() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        let _ = p.take_intents();
        let mut painter = Painter::new();
        p.paint(&mut painter, vp());

        let rects = painter
            .commands()
            .iter()
            .filter(|c| matches!(c, DrawCommand::Rect { .. }))
            .count();
        let texts = painter
            .commands()
            .iter()
            .filter(|c| matches!(c, DrawCommand::Text(_)))
            .count();
        // Background + accent bar + the selected-row highlight = at least 3.
        assert!(
            rects >= 3,
            "expected a selection highlight, got {rects} rects"
        );
        // Title + at least one agenda item row.
        assert!(texts >= 2, "expected title + rows, got {texts} texts");
    }

    #[test]
    fn paint_capture_picker_lists_templates() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Capture);
        let mut painter = Painter::new();
        p.paint(&mut painter, vp());
        // Title + the one configured template row.
        let texts = painter
            .commands()
            .iter()
            .filter(|c| matches!(c, DrawCommand::Text(_)))
            .count();
        assert_eq!(texts, 2);
    }

    #[test]
    fn hotkey_reopen_resets_filter_mode() {
        let mut p = popover();
        p.on_hotkey(HotkeyAction::Agenda);
        p.on_key(chord("/"));
        assert!(p.is_filtering());
        // Re-opening any surface clears the popover-local filter mode.
        p.on_hotkey(HotkeyAction::Agenda);
        assert!(!p.is_filtering());
    }
}
