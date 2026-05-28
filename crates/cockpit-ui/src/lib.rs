//! `cockpit-ui` — the headless view-model layer.
//!
//! A retained, plain-data view-model tree (layout, panes, file browser,
//! command palette, project launcher) that is a pure function of app state
//! and fully unit-testable without a window (spec §18.8).

pub mod completion;
pub mod file_browser;
pub mod file_finder;
pub mod hydration;
pub mod launcher;
pub mod palette;
pub mod prompt;

pub use completion::{CompletionItem, CompletionPopup};
pub use file_browser::{FileBrowser, FileBrowserAction, FileRow};
pub use file_finder::{FuzzyFinder, FuzzyMatch};
pub use hydration::{CompletedPhase, HydrationPhase, HydrationProgress};
pub use launcher::{Launcher, LauncherAction, LauncherIntent, LauncherSelection, RecentProject};
pub use palette::{Palette, PaletteEntry, PaletteMatch};
pub use prompt::ConfirmPrompt;

use cockpit_commands::{CommandError, CommandId, KeyChord, KeyChordParseError, Keymap, Modifiers};
use cockpit_config::GlobalKeys;

/// Default file-browser pane width from spec §12.
pub const DEFAULT_LEFT_WIDTH: u32 = 260;
/// Default terminal pane width from spec §12.
pub const DEFAULT_RIGHT_WIDTH: u32 = 480;
/// Minimum practical editor width before side panes are compressed.
pub const MIN_CENTER_WIDTH: u32 = 160;

/// One of the primary workspace panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Files,
    Editor,
    Terminal,
}

/// A pixel rectangle in logical coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Construct a rectangle.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// User/project-persisted workspace layout preferences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPreferences {
    pub left_width: u32,
    pub right_width: u32,
    pub files_visible: bool,
    pub terminal_visible: bool,
}

impl Default for LayoutPreferences {
    fn default() -> Self {
        Self {
            left_width: DEFAULT_LEFT_WIDTH,
            right_width: DEFAULT_RIGHT_WIDTH,
            files_visible: true,
            terminal_visible: true,
        }
    }
}

/// Headless workspace layout and focus model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLayout {
    prefs: LayoutPreferences,
    focused: PaneId,
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self {
            prefs: LayoutPreferences::default(),
            focused: PaneId::Editor,
        }
    }
}

impl WorkspaceLayout {
    /// Create the default layout.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current persisted preferences.
    pub fn preferences(&self) -> &LayoutPreferences {
        &self.prefs
    }

    /// Replace persisted preferences. Focus is corrected if the focused pane is
    /// no longer visible.
    pub fn set_preferences(&mut self, prefs: LayoutPreferences) {
        self.prefs = prefs;
        self.correct_focus();
    }

    /// Currently focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Focus a pane, making it visible if needed.
    pub fn focus(&mut self, pane: PaneId) {
        match pane {
            PaneId::Files => self.prefs.files_visible = true,
            PaneId::Terminal => self.prefs.terminal_visible = true,
            PaneId::Editor => {}
        }
        self.focused = pane;
    }

    /// Toggle the file browser pane.
    pub fn toggle_files(&mut self) {
        self.prefs.files_visible = !self.prefs.files_visible;
        self.correct_focus();
    }

    /// Toggle the terminal pane.
    pub fn toggle_terminal(&mut self) {
        self.prefs.terminal_visible = !self.prefs.terminal_visible;
        self.correct_focus();
    }

    /// Update persisted file-browser width.
    pub fn set_left_width(&mut self, width: u32) {
        self.prefs.left_width = width;
    }

    /// Update persisted terminal width.
    pub fn set_right_width(&mut self, width: u32) {
        self.prefs.right_width = width;
    }

    /// Compute pane rectangles for a viewport.
    pub fn compute(&self, viewport_width: u32, viewport_height: u32) -> ComputedLayout {
        let mut left_width = if self.prefs.files_visible {
            self.prefs.left_width
        } else {
            0
        };
        let mut right_width = if self.prefs.terminal_visible {
            self.prefs.right_width
        } else {
            0
        };

        let max_side_total = viewport_width.saturating_sub(MIN_CENTER_WIDTH);
        let side_total = left_width + right_width;
        if side_total > max_side_total {
            left_width = left_width.saturating_mul(max_side_total) / side_total;
            right_width = max_side_total.saturating_sub(left_width);
        }

        let editor_x = left_width;
        let editor_width = viewport_width.saturating_sub(left_width + right_width);
        let terminal_x = viewport_width.saturating_sub(right_width);

        ComputedLayout {
            files: (left_width > 0).then_some(Rect::new(0, 0, left_width, viewport_height)),
            editor: Rect::new(editor_x, 0, editor_width, viewport_height),
            terminal: (right_width > 0).then_some(Rect::new(
                terminal_x,
                0,
                right_width,
                viewport_height,
            )),
            focused: self.focused,
        }
    }

    fn correct_focus(&mut self) {
        if self.focused == PaneId::Files && !self.prefs.files_visible {
            self.focused = PaneId::Editor;
        }
        if self.focused == PaneId::Terminal && !self.prefs.terminal_visible {
            self.focused = PaneId::Editor;
        }
    }
}

/// Computed pane rectangles for one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputedLayout {
    pub files: Option<Rect>,
    pub editor: Rect,
    pub terminal: Option<Rect>,
    pub focused: PaneId,
}

/// Initial command IDs used by the global input router.
pub mod command_ids {
    pub const FOCUS_FILES: &str = "pane.focus_files";
    pub const FOCUS_EDITOR: &str = "pane.focus_editor";
    pub const FOCUS_TERMINAL: &str = "pane.focus_terminal";
    pub const TOGGLE_TERMINAL: &str = "pane.toggle_terminal";
    pub const TOGGLE_FILES: &str = "pane.toggle_files";
    pub const COMMAND_PALETTE: &str = "palette.open";
    pub const FUZZY_OPEN: &str = "file.fuzzy_open";
    pub const SAVE: &str = "file.save";
}

/// Result of routing one key chord.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutedInput {
    Command(CommandId),
    TerminalPassthrough(KeyChord),
    Unhandled(KeyChord),
}

/// Pure key input router for the global shortcuts in spec §12.
#[derive(Debug, Default)]
pub struct InputRouter {
    global: Keymap,
}

impl InputRouter {
    /// Build a router from the configured global keys.
    pub fn from_global_keys(keys: &GlobalKeys) -> Result<Self, InputRouterError> {
        let mut global = Keymap::new();
        bind_configured(&mut global, &keys.focus_files, command_ids::FOCUS_FILES)?;
        bind_configured(&mut global, &keys.focus_editor, command_ids::FOCUS_EDITOR)?;
        bind_configured(
            &mut global,
            &keys.focus_terminal,
            command_ids::FOCUS_TERMINAL,
        )?;
        bind_configured(
            &mut global,
            &keys.toggle_terminal,
            command_ids::TOGGLE_TERMINAL,
        )?;
        bind_configured(&mut global, &keys.toggle_files, command_ids::TOGGLE_FILES)?;
        bind_configured(
            &mut global,
            &keys.command_palette,
            command_ids::COMMAND_PALETTE,
        )?;
        bind_configured(&mut global, &keys.fuzzy_open, command_ids::FUZZY_OPEN)?;
        global
            .bind(
                KeyChord::single("s", Modifiers::CTRL),
                CommandId::from(command_ids::SAVE),
            )
            .map_err(InputRouterError::Command)?;
        Ok(Self { global })
    }

    /// Bind an extra chord → command id on top of the configured globals.
    /// Used by v0.8 tool-pane recipes that ship their own keybind. Parse
    /// errors propagate so the caller can surface a status warning.
    pub fn bind_extra_chord(
        &mut self,
        chord: &str,
        command: impl Into<CommandId>,
    ) -> Result<(), InputRouterError> {
        let chord = chord
            .parse::<KeyChord>()
            .map_err(|source| InputRouterError::ParseChord {
                chord: chord.to_string(),
                source,
            })?;
        self.global
            .bind(chord, command.into())
            .map_err(InputRouterError::Command)
    }

    /// Route a key chord based on the currently focused pane.
    pub fn route(&self, focused: PaneId, chord: KeyChord) -> RoutedInput {
        if let Some(command) = self.global.resolve(&chord) {
            return RoutedInput::Command(command.clone());
        }

        if focused == PaneId::Terminal {
            RoutedInput::TerminalPassthrough(chord)
        } else {
            RoutedInput::Unhandled(chord)
        }
    }
}

fn bind_configured(
    keymap: &mut Keymap,
    chord: &str,
    command: &'static str,
) -> Result<(), InputRouterError> {
    let chord = chord
        .parse::<KeyChord>()
        .map_err(|source| InputRouterError::ParseChord {
            chord: chord.to_string(),
            source,
        })?;
    keymap
        .bind(chord, CommandId::from(command))
        .map_err(InputRouterError::Command)
}

/// Input router construction error.
#[derive(Debug)]
pub enum InputRouterError {
    ParseChord {
        chord: String,
        source: KeyChordParseError,
    },
    Command(CommandError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_matches_spec_widths() {
        let layout = WorkspaceLayout::new().compute(1200, 800);
        assert_eq!(layout.files, Some(Rect::new(0, 0, 260, 800)));
        assert_eq!(layout.editor, Rect::new(260, 0, 460, 800));
        assert_eq!(layout.terminal, Some(Rect::new(720, 0, 480, 800)));
        assert_eq!(layout.focused, PaneId::Editor);
    }

    #[test]
    fn hidden_side_panes_give_space_to_editor() {
        let mut layout = WorkspaceLayout::new();
        layout.toggle_files();
        layout.toggle_terminal();

        let computed = layout.compute(900, 600);
        assert_eq!(computed.files, None);
        assert_eq!(computed.editor, Rect::new(0, 0, 900, 600));
        assert_eq!(computed.terminal, None);
    }

    #[test]
    fn narrow_viewport_compresses_side_panes_and_preserves_center_minimum() {
        let layout = WorkspaceLayout::new().compute(600, 400);

        assert_eq!(layout.editor.width, MIN_CENTER_WIDTH);
        assert_eq!(
            layout.files.unwrap().width + layout.editor.width + layout.terminal.unwrap().width,
            600
        );
    }

    #[test]
    fn focus_makes_side_pane_visible() {
        let mut layout = WorkspaceLayout::new();
        layout.toggle_files();
        assert!(!layout.preferences().files_visible);

        layout.focus(PaneId::Files);
        assert!(layout.preferences().files_visible);
        assert_eq!(layout.focused(), PaneId::Files);
    }

    #[test]
    fn hiding_focused_side_pane_returns_focus_to_editor() {
        let mut layout = WorkspaceLayout::new();
        layout.focus(PaneId::Terminal);
        layout.toggle_terminal();

        assert_eq!(layout.focused(), PaneId::Editor);
        assert!(!layout.preferences().terminal_visible);
    }

    #[test]
    fn custom_widths_are_persisted_preferences() {
        let mut layout = WorkspaceLayout::new();
        layout.set_left_width(300);
        layout.set_right_width(360);

        let computed = layout.compute(1000, 500);
        assert_eq!(computed.files.unwrap().width, 300);
        assert_eq!(computed.terminal.unwrap().width, 360);
        assert_eq!(computed.editor.width, 340);
    }

    #[test]
    fn input_router_maps_global_shortcuts_to_commands() {
        let router = InputRouter::from_global_keys(&GlobalKeys::default()).unwrap();

        assert_eq!(
            router.route(PaneId::Editor, "Ctrl+h".parse().unwrap()),
            RoutedInput::Command(CommandId::from(command_ids::FOCUS_FILES))
        );
        assert_eq!(
            router.route(PaneId::Editor, "Ctrl+Shift+p".parse().unwrap()),
            RoutedInput::Command(CommandId::from(command_ids::COMMAND_PALETTE))
        );
        assert_eq!(
            router.route(PaneId::Editor, "Ctrl+s".parse().unwrap()),
            RoutedInput::Command(CommandId::from(command_ids::SAVE))
        );
    }

    #[test]
    fn terminal_focus_passes_non_global_keys_through() {
        let router = InputRouter::from_global_keys(&GlobalKeys::default()).unwrap();
        let chord = "a".parse::<KeyChord>().unwrap();

        assert_eq!(
            router.route(PaneId::Terminal, chord.clone()),
            RoutedInput::TerminalPassthrough(chord)
        );
    }

    #[test]
    fn terminal_focus_still_intercepts_global_keys() {
        let router = InputRouter::from_global_keys(&GlobalKeys::default()).unwrap();

        assert_eq!(
            router.route(PaneId::Terminal, "Ctrl+l".parse().unwrap()),
            RoutedInput::Command(CommandId::from(command_ids::FOCUS_TERMINAL))
        );
    }

    #[test]
    fn editor_focus_leaves_non_global_keys_unhandled() {
        let router = InputRouter::from_global_keys(&GlobalKeys::default()).unwrap();
        let chord = "a".parse::<KeyChord>().unwrap();

        assert_eq!(
            router.route(PaneId::Editor, chord.clone()),
            RoutedInput::Unhandled(chord)
        );
    }

    #[cfg(feature = "ui-smoke")]
    #[test]
    fn ui_smoke_view_models_cover_launcher_layout_browser_and_keys() {
        let launcher = Launcher::new(vec![RecentProject::new(
            "file-tree",
            cockpit_testkit::fixture_path("file-tree"),
        )]);
        assert_eq!(launcher.selection(), LauncherSelection::Recent(0));
        assert!(matches!(launcher.activate(), LauncherIntent::OpenRecent(0)));

        let layout = WorkspaceLayout::new().compute(1200, 800);
        assert!(layout.files.is_some());
        assert_eq!(layout.editor.height, 800);
        assert!(layout.terminal.is_some());

        let tree = cockpit_project::FileTree::load(cockpit_testkit::fixture_path("file-tree"))
            .expect("load fixture tree");
        let mut browser = FileBrowser::new(tree);
        assert_eq!(browser.selected().unwrap().name, "src");
        assert_eq!(browser.activate().unwrap(), FileBrowserAction::Toggled);
        assert!(browser.rows().iter().any(|row| row.name == "lib.rs"));

        let router = InputRouter::from_global_keys(&GlobalKeys::default()).unwrap();
        assert_eq!(
            router.route(PaneId::Editor, "Ctrl+h".parse().unwrap()),
            RoutedInput::Command(CommandId::from(command_ids::FOCUS_FILES))
        );
        assert_eq!(
            router.route(PaneId::Terminal, "x".parse().unwrap()),
            RoutedInput::TerminalPassthrough("x".parse().unwrap())
        );
    }
}
