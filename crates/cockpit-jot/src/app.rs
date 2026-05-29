//! The headless jot controller (M12.6 core).
//!
//! `JotController` is the backend-free brain of the tray app: it owns the live
//! [`OrgRoot`], the capture/agenda/overview view-models, and the tray menu, and
//! turns **events** (a global hotkey fired, a tray item chosen, a key pressed
//! in the popover) into **intents** (show/dismiss the popover, write a file,
//! open a location in the cockpit, quit). The winit popover, the `tray-icon`
//! menu, and the `global-hotkey` registration that produce those events and
//! execute those intents are the binary's `main.rs` glue (behind `ui-smoke`);
//! this layer is pure and fully testable.
//!
//! Non-determinism is injected: the controller is constructed with a
//! [`NowStamp`], so capture timestamps and the agenda's "today" are
//! deterministic in tests.

use std::path::PathBuf;

use cockpit_org::{CaptureContext, NowStamp, OrgConfig, OrgDate, OrgRoot, apply_capture};
use cockpit_tray::{Menu, MenuItem, MenuItemId, TrayEvent};
use cockpit_ui::{AgendaMode, AgendaView, CaptureView, OrgListView};

/// The three things a global hotkey can ask the jot app to do (M12.6 default
/// chords: `Ctrl+O`, `Ctrl+Alt+A`, `Ctrl+Alt+O`). The binary maps fired
/// hotkey ids to these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    Capture,
    Agenda,
    Overview,
}

/// What the popover is currently showing.
pub enum Surface {
    /// Popover hidden; only the tray icon + watcher are live.
    Hidden,
    Capture(CaptureView),
    Agenda(AgendaView),
    Overview(OrgListView),
}

impl Surface {
    /// Discriminant name, handy for tests/painters.
    pub fn name(&self) -> &'static str {
        match self {
            Surface::Hidden => "hidden",
            Surface::Capture(_) => "capture",
            Surface::Agenda(_) => "agenda",
            Surface::Overview(_) => "overview",
        }
    }
}

/// A side effect for the binary to carry out. The controller never touches the
/// window, the filesystem, or the IPC socket directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JotIntent {
    /// Show the popover (a surface was just opened).
    ShowPopover,
    /// Dismiss the popover, returning focus to wherever it was.
    DismissPopover,
    /// Persist `source` to `path` (capture commit / repeater bump). The
    /// controller has already updated its in-memory root to match.
    WriteFile { path: PathBuf, source: String },
    /// Ask the cockpit (over IPC) to open `path` at `line`.
    OpenInCockpit { path: PathBuf, line: usize },
    /// Quit the tray app.
    Quit,
}

/// Tray menu item ids (stable; the binary builds the OS menu from
/// [`JotController::tray_menu`]).
pub mod menu_ids {
    pub const CAPTURE: &str = "capture";
    pub const AGENDA: &str = "agenda";
    pub const INBOX: &str = "inbox";
    pub const ROOT: &str = "root";
    pub const SETTINGS: &str = "settings";
    pub const QUIT: &str = "quit";
}

/// The jot app's headless state machine.
pub struct JotController {
    config: OrgConfig,
    root: OrgRoot,
    now: NowStamp,
    surface: Surface,
    /// Context applied to the next template expansion: `%a` (annotation) and
    /// `%i` (initial content). A bare hotkey capture has none; a capture
    /// triggered from the cockpit (over IPC) or the CLI supplies the editor's
    /// `path:line` and selection here. Scoped to the capture session — every
    /// `open_capture*` resets it.
    pending_ctx: CaptureContext,
}

impl JotController {
    /// Build a controller over a pre-loaded `root`, the parsed `config`, and an
    /// injected `now`.
    pub fn new(config: OrgConfig, root: OrgRoot, now: NowStamp) -> Self {
        JotController {
            config,
            root,
            now,
            surface: Surface::Hidden,
            pending_ctx: CaptureContext::default(),
        }
    }

    /// The reference day for the agenda (the date component of `now`).
    pub fn today(&self) -> OrgDate {
        self.now.date
    }

    /// The current popover surface.
    pub fn surface(&self) -> &Surface {
        &self.surface
    }

    /// The live in-memory org root.
    pub fn root(&self) -> &OrgRoot {
        &self.root
    }

    /// The tray menu (plan M12.6: Capture…, Agenda, Open inbox, Open root in
    /// cockpit, Settings, Quit).
    pub fn tray_menu(&self) -> Menu {
        Menu::new([
            MenuItem::action(menu_ids::CAPTURE, "Capture…"),
            MenuItem::action(menu_ids::AGENDA, "Agenda"),
            MenuItem::action(menu_ids::INBOX, "Open inbox"),
            MenuItem::action(menu_ids::ROOT, "Open root in cockpit"),
            MenuItem::separator(),
            MenuItem::action(menu_ids::SETTINGS, "Settings"),
            MenuItem::action(menu_ids::QUIT, "Quit"),
        ])
    }

    // ---- event entry points -------------------------------------------------

    /// React to a global hotkey.
    pub fn on_hotkey(&mut self, action: HotkeyAction) -> Vec<JotIntent> {
        match action {
            HotkeyAction::Capture => self.open_capture(),
            HotkeyAction::Agenda => self.open_agenda(),
            HotkeyAction::Overview => self.open_overview(),
        }
    }

    /// React to a tray event.
    pub fn on_tray(&mut self, event: TrayEvent) -> Vec<JotIntent> {
        match event {
            // Left-click opens the capture picker (the most common action).
            TrayEvent::LeftClick => self.open_capture(),
            TrayEvent::MenuItem(id) => self.on_menu(&id),
        }
    }

    fn on_menu(&mut self, id: &MenuItemId) -> Vec<JotIntent> {
        match id.0.as_str() {
            menu_ids::CAPTURE => self.open_capture(),
            menu_ids::AGENDA => self.open_agenda(),
            menu_ids::INBOX => self.open_in_cockpit("inbox.org", 0),
            menu_ids::ROOT => vec![JotIntent::OpenInCockpit {
                path: self.root.root_dir.clone(),
                line: 0,
            }],
            menu_ids::SETTINGS => self.open_in_cockpit("../.config/cockpit/org.toml", 0),
            menu_ids::QUIT => vec![JotIntent::Quit],
            _ => Vec::new(),
        }
    }

    // ---- surface openers ----------------------------------------------------

    fn open_capture(&mut self) -> Vec<JotIntent> {
        // A bare hotkey/tray capture carries no editor context.
        self.pending_ctx = CaptureContext::default();
        self.surface = Surface::Capture(CaptureView::from_config(&self.config));
        vec![JotIntent::ShowPopover]
    }

    /// Open the capture picker with editor context (`%a` annotation, `%i`
    /// initial content) for the template expansion. Used by the cockpit-driven
    /// (IPC) and CLI capture paths, where a source location / selection is
    /// known; the bare hotkey uses [`HotkeyAction::Capture`] instead.
    pub fn open_capture_with(&mut self, ctx: CaptureContext) -> Vec<JotIntent> {
        self.pending_ctx = ctx;
        self.surface = Surface::Capture(CaptureView::from_config(&self.config));
        vec![JotIntent::ShowPopover]
    }

    fn open_agenda(&mut self) -> Vec<JotIntent> {
        let view = AgendaView::build(&self.root, self.today(), AgendaMode::Today);
        self.surface = Surface::Agenda(view);
        vec![JotIntent::ShowPopover]
    }

    fn open_overview(&mut self) -> Vec<JotIntent> {
        self.surface = Surface::Overview(OrgListView::build(&self.root));
        vec![JotIntent::ShowPopover]
    }

    /// Dismiss the popover (ESC / focus loss).
    pub fn dismiss(&mut self) -> Vec<JotIntent> {
        self.surface = Surface::Hidden;
        vec![JotIntent::DismissPopover]
    }

    // ---- capture flow -------------------------------------------------------

    /// In the capture picker, choose the template with picker key `key`.
    /// Returns `true` if a template matched (the surface stays on capture, now
    /// in the editing phase).
    pub fn capture_pick(&mut self, key: &str) -> bool {
        let now = self.now.clone();
        let ctx = self.pending_ctx.clone();
        if let Surface::Capture(view) = &mut self.surface {
            view.pick(key, &now, &ctx)
        } else {
            false
        }
    }

    /// Insert `text` at the capture editor's cursor (the `%?` slot after a
    /// pick). No-op unless an editing capture is open.
    pub fn capture_insert_str(&mut self, text: &str) {
        if let Surface::Capture(view) = &mut self.surface {
            view.insert_str(text);
        }
    }

    /// Commit the capture being edited: expand → file under the template's
    /// target → persist. Returns the write + dismiss intents, or empty if the
    /// surface isn't an editing capture.
    pub fn capture_commit(&mut self) -> Vec<JotIntent> {
        let Surface::Capture(view) = &self.surface else {
            return Vec::new();
        };
        let Some(commit) = view.commit() else {
            return Vec::new();
        };

        let path = self.resolve(&commit.file);
        let current = self
            .root
            .file(&path)
            .map(|f| f.source.clone())
            .unwrap_or_default();
        let outcome = apply_capture(&current, &commit.target, &commit.entry, &self.now);

        // Keep the live root in sync, then ask the binary to persist.
        self.root.insert(path.clone(), outcome.source.clone());
        self.surface = Surface::Hidden;
        vec![
            JotIntent::WriteFile {
                path,
                source: outcome.source,
            },
            JotIntent::DismissPopover,
        ]
    }

    // ---- agenda flow --------------------------------------------------------

    /// Cycle the agenda mode (Tab). No-op unless the agenda is open.
    pub fn agenda_cycle_mode(&mut self) {
        let today = self.today();
        if let Surface::Agenda(view) = &mut self.surface {
            view.cycle_mode(&self.root, today);
        }
    }

    /// Move the agenda/overview cursor down.
    pub fn cursor_down(&mut self) {
        match &mut self.surface {
            Surface::Agenda(v) => v.move_down(),
            Surface::Overview(v) => v.move_down(),
            _ => {}
        }
    }

    /// Move the agenda/overview cursor up.
    pub fn cursor_up(&mut self) {
        match &mut self.surface {
            Surface::Agenda(v) => v.move_up(),
            Surface::Overview(v) => v.move_up(),
            _ => {}
        }
    }

    /// Activate the selected agenda/overview row: jump into the cockpit at the
    /// underlying `.org` location and dismiss the popover.
    pub fn activate_selection(&mut self) -> Vec<JotIntent> {
        let target = match &self.surface {
            Surface::Agenda(v) => v.jump_target().map(own),
            Surface::Overview(v) => v.jump_target().map(own),
            _ => None,
        };
        match target {
            Some((path, line)) => {
                self.surface = Surface::Hidden;
                vec![
                    JotIntent::OpenInCockpit { path, line },
                    JotIntent::DismissPopover,
                ]
            }
            None => Vec::new(),
        }
    }

    // ---- helpers ------------------------------------------------------------

    fn open_in_cockpit(&self, rel: &str, line: usize) -> Vec<JotIntent> {
        vec![JotIntent::OpenInCockpit {
            path: self.resolve(rel),
            line,
        }]
    }

    /// Resolve a config-relative file (e.g. `inbox.org`) against the org root.
    fn resolve(&self, rel: &str) -> PathBuf {
        self.root.root_dir.join(rel)
    }
}

fn own((path, line): (&std::path::Path, usize)) -> (PathBuf, usize) {
    (path.to_path_buf(), line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_org::{CaptureTarget, CaptureTemplate, OrgTime};
    use cockpit_tray::MenuItemId;

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

    fn controller() -> JotController {
        let root = OrgRoot::from_files(
            "/org",
            [
                ("/org/inbox.org", "* Tasks\n"),
                (
                    "/org/tasks.org",
                    "* TODO ship it\nSCHEDULED: <2026-05-29 Fri>\n",
                ),
            ],
        );
        JotController::new(config(), root, now())
    }

    #[test]
    fn hotkey_opens_surfaces() {
        let mut c = controller();
        assert_eq!(c.surface().name(), "hidden");

        assert_eq!(
            c.on_hotkey(HotkeyAction::Agenda),
            vec![JotIntent::ShowPopover]
        );
        assert_eq!(c.surface().name(), "agenda");

        c.on_hotkey(HotkeyAction::Capture);
        assert_eq!(c.surface().name(), "capture");

        c.on_hotkey(HotkeyAction::Overview);
        assert_eq!(c.surface().name(), "overview");
    }

    #[test]
    fn tray_menu_has_six_actions_and_routes() {
        let mut c = controller();
        assert_eq!(c.tray_menu().action_ids().count(), 6);

        // Quit.
        assert_eq!(
            c.on_tray(TrayEvent::MenuItem(MenuItemId::from(menu_ids::QUIT))),
            vec![JotIntent::Quit]
        );
        // Open inbox -> jump into the cockpit at inbox.org.
        let intents = c.on_tray(TrayEvent::MenuItem(MenuItemId::from(menu_ids::INBOX)));
        assert_eq!(
            intents,
            vec![JotIntent::OpenInCockpit {
                path: PathBuf::from("/org/inbox.org"),
                line: 0,
            }]
        );
        // Left click opens capture.
        c.on_tray(TrayEvent::LeftClick);
        assert_eq!(c.surface().name(), "capture");
    }

    #[test]
    fn capture_flow_writes_file_and_syncs_root() {
        let mut c = controller();
        c.on_hotkey(HotkeyAction::Capture);
        assert!(c.capture_pick("t"));

        // Type a title into the %? slot.
        c.capture_insert_str("buy milk");

        let intents = c.capture_commit();
        // The entry is filed under "Tasks" in inbox.org (demoted to level 2).
        match &intents[0] {
            JotIntent::WriteFile { path, source } => {
                assert_eq!(path, &PathBuf::from("/org/inbox.org"));
                assert_eq!(source, "* Tasks\n** TODO buy milk\n");
            }
            other => panic!("expected WriteFile, got {other:?}"),
        }
        assert_eq!(intents[1], JotIntent::DismissPopover);
        // Live root reflects the capture.
        assert_eq!(
            c.root().file("/org/inbox.org").unwrap().source,
            "* Tasks\n** TODO buy milk\n"
        );
        assert_eq!(c.surface().name(), "hidden");
    }

    #[test]
    fn agenda_enter_jumps_into_cockpit() {
        let mut c = controller();
        c.on_hotkey(HotkeyAction::Agenda);
        let intents = c.activate_selection();
        assert_eq!(
            intents,
            vec![
                JotIntent::OpenInCockpit {
                    path: PathBuf::from("/org/tasks.org"),
                    line: 0,
                },
                JotIntent::DismissPopover,
            ]
        );
        assert_eq!(c.surface().name(), "hidden");
    }

    #[test]
    fn agenda_tab_cycles_mode() {
        let mut c = controller();
        c.on_hotkey(HotkeyAction::Agenda);
        if let Surface::Agenda(v) = c.surface() {
            assert_eq!(v.mode(), AgendaMode::Today);
        }
        c.agenda_cycle_mode();
        if let Surface::Agenda(v) = c.surface() {
            assert_eq!(v.mode(), AgendaMode::Next7Days);
        } else {
            panic!("expected agenda surface");
        }
    }

    #[test]
    fn dismiss_hides_surface() {
        let mut c = controller();
        c.on_hotkey(HotkeyAction::Agenda);
        assert_eq!(c.dismiss(), vec![JotIntent::DismissPopover]);
        assert_eq!(c.surface().name(), "hidden");
    }

    #[test]
    fn capture_commit_without_capture_surface_is_noop() {
        let mut c = controller();
        assert!(c.capture_commit().is_empty());
        assert!(!c.capture_pick("t")); // not in capture surface
    }

    #[test]
    fn capture_insert_str_is_noop_off_capture_surface() {
        let mut c = controller();
        // No capture open: inserting is a silent no-op, not a panic.
        c.capture_insert_str("ignored");
        assert_eq!(c.surface().name(), "hidden");
    }

    #[test]
    fn open_capture_with_context_expands_annotation() {
        // A template with %a should pick up the supplied annotation; the bare
        // hotkey path leaves it empty.
        let mut c = JotController::new(
            OrgConfig {
                root: Some("/org".into()),
                default_todo_keywords: vec!["TODO".into(), "DONE".into()],
                capture: vec![CaptureTemplate {
                    key: "n".into(),
                    name: "Note".into(),
                    target: CaptureTarget {
                        file: "notes.org".into(),
                        under: None,
                        datetree: false,
                    },
                    template: "* %? from %a".into(),
                }],
            },
            OrgRoot::from_files("/org", [("/org/notes.org", "")]),
            now(),
        );

        c.open_capture_with(CaptureContext {
            annotation: Some("src/lib.rs:10".into()),
            ..Default::default()
        });
        assert!(c.capture_pick("n"));
        if let Surface::Capture(v) = c.surface() {
            assert_eq!(v.buffer(), "*  from src/lib.rs:10");
        } else {
            panic!("expected capture surface");
        }

        // The bare hotkey path resets context: %a expands to empty.
        c.on_hotkey(HotkeyAction::Capture);
        assert!(c.capture_pick("n"));
        if let Surface::Capture(v) = c.surface() {
            assert_eq!(v.buffer(), "*  from ");
        }
    }

    #[test]
    fn capture_pick_unknown_key_stays_in_picker() {
        let mut c = controller();
        c.on_hotkey(HotkeyAction::Capture);
        assert!(!c.capture_pick("zzz"));
        // Still on the capture surface, just nothing picked.
        assert_eq!(c.surface().name(), "capture");
    }
}
