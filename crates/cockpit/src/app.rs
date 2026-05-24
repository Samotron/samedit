//! The application shell — Track D wire-up.
//!
//! [`AppModel`] is the headless, plain-data application state: the project,
//! the file-browser view-model, the workspace layout, the global input router,
//! and the open editor document. It turns key chords into state changes and
//! paints itself into a [`Painter`]. [`AppShell`] is the thin [`CockpitApp`]
//! adapter the windowing harness drives — all real logic lives in [`AppModel`],
//! so it stays testable without a window (AGENTS §2).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use cockpit_analytics::detect::scan_models_dir;
use cockpit_analytics::{
    AnalyticsProject, BuildPlan, Materialisation, build_plan, detect_analytics_project,
};
use cockpit_commands::{KeyChord, Modifiers};
use cockpit_config::{GlobalKeys, ZellijLayout};
use cockpit_editor::vim::{Key as VimKey, Mode};
use cockpit_editor::{
    Editor, EditorSignal, HighlightKind, HighlightSpan, Language, nearest_test_name,
};
use cockpit_lsp::{
    Diagnostic, DiagnosticSeverity, LspClient, PublishDiagnosticsParams, RecvMessage, Response,
    ServerConfig,
};
use cockpit_mux::{
    PrefixDispatcher, Rect as MuxRect, Session as MuxSession, SplitDirection,
    command_ids as mux_command_ids,
};
use cockpit_notebook::{
    CellKind, Notebook, is_notebook_source, parse_notebook, parse_quarto, quarto_render_spec,
};
use cockpit_project::{
    FileNodeKind, FileSystem, FileTree, FormatPlan, KnownFormatter,
    PathBinaryLookup as FormatPathLookup, ProcessRunner, ProcessSpec, ProjectCache,
    ProjectDetection, StdFileSystem, StdProcessRunner, mise_exec_command, plan_format,
    project_cache_path, render_format_task_snippet, walk_project_files,
};
use cockpit_render::theme::Color;
use cockpit_render::{
    CockpitApp, MouseButton, Painter, PointerPosition, Rect as RenderRect, RedrawHandle, Theme,
    Viewport,
};
use cockpit_sql::{DuckDbEngine, GgsqlEngine, SqlEngine, statement_targets_ggsql};
use cockpit_terminal::bridge::{detect_paths_in_grid, paste_to_terminal, render_document_path};
use cockpit_terminal::live::{LiveTerminal, WakeFn};
use cockpit_terminal::path_detect::detect_paths;
use cockpit_terminal::pty::PtyDimensions;
use cockpit_terminal::session::TerminalStatus;
use cockpit_terminal::zellij::{LaunchPlan, PathBinaryLookup, ShellProfile, plan_launch};
use cockpit_ui::{
    CompletionItem, CompletionPopup, ComputedLayout, ConfirmPrompt, FileBrowser, FileBrowserAction,
    FuzzyFinder, InputRouter, Palette, PaletteEntry, PaneId, Rect as UiRect, RoutedInput,
    WorkspaceLayout, command_ids,
};

/// Logical layout metrics. The painter scales these by the display factor.
pub const TOP_BAR_H: f32 = 30.0;
pub const HEADER_H: f32 = 24.0;
pub const ROW_H: f32 = 20.0;
pub const FONT: f32 = 13.0;
pub const PAD: f32 = 8.0;
pub const GUTTER_W: f32 = 52.0;
pub const INDENT_W: f32 = 14.0;
/// Monospace advance estimate, as a fraction of the font size.
pub const CHAR_W_RATIO: f32 = 0.6;

/// Command id for "quit the application" — handled directly by the shell.
const APP_QUIT: &str = "app.quit";
/// Command id for "pick and run a mise task".
const MISE_RUN_TASK: &str = "mise.run_task";
/// Command id for "open a file path from the terminal output".
const TERMINAL_OPEN_PATH: &str = "terminal.open_path";
/// Command id for "send the current file path to the terminal" (spec §17).
const TERMINAL_SEND_FILE_PATH: &str = "terminal.send_file_path";
/// Command id for "send the editor's visual selection to the terminal" (spec §17).
const TERMINAL_SEND_SELECTION: &str = "terminal.send_selection";
/// Command id for "run the project's mise `test` task" (spec §16).
const TEST_RUN_ALL: &str = "test.run_all";
/// Command id for "run the `test` task targeting the current file" (spec §16).
const TEST_RUN_CURRENT_FILE: &str = "test.run_current_file";
/// Command id for "run the `test` task targeting the nearest test" (spec §16).
const TEST_RUN_NEAREST: &str = "test.run_nearest";
/// Command id for "summarise the recent key chord ring buffer" (spec §18.13).
const DEBUG_SHOW_KEY_EVENTS: &str = "debug.show_key_events";
/// Command id for "summarise the recent command dispatch log" (spec §18.13).
const DEBUG_SHOW_COMMAND_LOG: &str = "debug.show_command_log";
/// Command id for "summarise the current pane tree" (spec §18.13).
const DEBUG_SHOW_PANE_TREE: &str = "debug.show_pane_tree";
/// Command id for "summarise the project detection result" (spec §18.13).
const DEBUG_SHOW_PROJECT_STATE: &str = "debug.show_project_state";
/// Command id for "reload the user config" (spec §18.13).
const DEBUG_RELOAD_CONFIG: &str = "debug.reload_config";
/// Command id for "summarise the recorded startup-phase trace"
/// (v0.6 M6.7).
const DEBUG_SHOW_STARTUP_TRACE: &str = "debug.show_startup_trace";
/// Command id for "go to the symbol's definition under the cursor" (M4.2).
const LSP_GOTO_DEFINITION: &str = "lsp.goto_definition";
/// Command id for "show hover information for the symbol under the cursor" (M4.2).
const LSP_SHOW_HOVER: &str = "lsp.show_hover";
/// Command id for "rename the symbol under the cursor" (M4.3a).
const LSP_RENAME: &str = "lsp.rename";
/// Command id for manually requesting completions (M4.3b).
const LSP_COMPLETION: &str = "lsp.completion";
/// Command id for "apply the first quick-fix for the current diagnostic" (M4.5).
const LSP_CODE_ACTION: &str = "lsp.code_action";
/// Command id for "format the open document" (M4.4). The implementation
/// chooses between a mise task, a prompt to add one, or LSP `formatting`
/// — see [`AppModel::request_format`] for the resolution rules.
const EDITOR_FORMAT: &str = "editor.format";
/// Command id for "toggle the per-session format-on-save preference" (M4.4).
const EDITOR_TOGGLE_FORMAT_ON_SAVE: &str = "editor.toggle_format_on_save";
/// Command id for "execute the active notebook cell" (v0.5 M5.3 wire-up).
const NOTEBOOK_RUN_ACTIVE_CELL: &str = "notebook.run_active_cell";
/// Command id for "advance the notebook cursor to the next cell".
const NOTEBOOK_NEXT_CELL: &str = "notebook.next_cell";
/// Command id for "move the notebook cursor to the previous cell".
const NOTEBOOK_PREVIOUS_CELL: &str = "notebook.previous_cell";
/// Command id for "insert a fresh cell below the active one".
const NOTEBOOK_INSERT_CELL_BELOW: &str = "notebook.insert_cell_below";
/// Command id for "build every materialisation in the dbt-lite project"
/// (v0.5 M5.8 wire-up).
const MODELS_BUILD_ALL: &str = "models.build_all";
/// Command id for "show the dbt-lite DAG summary in the status line".
const MODELS_SHOW_DAG: &str = "models.show_dag";
/// Command id for "render the active Quarto document via `quarto
/// render`" (v0.5 M5.Q3 wire-up).
const QUARTO_RENDER: &str = "quarto.render";

/// Name of the mise task cockpit invokes for the Test: * palette entries.
/// Conventional default; M4 may make this configurable per project.
const TEST_TASK: &str = "test";
/// Status surfaced when the project has no `test` mise task wired up.
const TEST_TASK_MISSING: &str = "No `test` mise task — add one to your mise.toml.";
/// Cap on the diagnostics ring buffers (recent keys, recent commands).
const DEBUG_LOG_SIZE: usize = 32;
/// Hard cap on file size before cockpit will start a language server for it
/// (spec §19 — never for huge files). 1 MiB matches the spec's "huge files"
/// posture; anything bigger keeps the LSP path cold.
const LSP_MAX_BYTES: usize = 1_048_576;

/// What activating a palette entry does — the palette is reused as both the
/// command palette and the mise-task picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    /// Entries are app commands dispatched through `run_command`.
    Commands,
    /// Entries are mise task names sent to the terminal.
    MiseTasks,
    /// Entries are `path:line:col` references opened in the editor.
    TerminalPaths,
}

/// The document open in the editor pane: an [`Editor`] plus its file path.
struct OpenDocument {
    editor: Editor,
    path: PathBuf,
    name: String,
}

/// What an in-flight LSP request was asking for, so its [`Response`] can be
/// routed to the right model update (M4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
enum LspPending {
    /// `textDocument/definition` — jump to the result Location/LocationLink.
    GotoDefinition,
    /// `textDocument/hover` — show the result in [`AppModel::hover`].
    Hover { path: PathBuf },
    /// `textDocument/codeAction` — apply the first returned quick-fix edit.
    CodeAction,
    /// `textDocument/prepareRename` — validate before issuing rename.
    PrepareRename {
        language: Language,
        path: PathBuf,
        line: usize,
        col: usize,
        new_name: String,
    },
    /// `textDocument/rename` — apply the returned workspace edit.
    Rename,
    /// `textDocument/completion` — show candidates in the completion popup.
    Completion,
    /// `textDocument/formatting` — apply the returned text edits (M4.4).
    /// `trigger` records whether the format came from a save (in which case
    /// the formatted buffer must be flushed back to disk on success).
    Formatting {
        path: PathBuf,
        trigger: FormatTrigger,
    },
}

/// Why a format request was issued (M4.4). Drives post-apply behaviour: a
/// save-triggered format flushes the formatted buffer back to disk, whereas
/// a manual `editor.format` leaves the buffer dirty so the user can review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormatTrigger {
    Manual,
    Save,
}

/// What confirming a [`ConfirmPrompt`] should do (M4.4 — and the seam any
/// future yes/no prompt plugs into).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptIntent {
    /// Append `[tasks.format]` to `mise.toml`, then retry formatting.
    AddFormatTask {
        snippet: String,
        retry_trigger: FormatTrigger,
    },
}

/// The most recently received hover result (M4.2). Cleared when a new
/// request comes back empty or when the document closes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HoverInfo {
    /// Document the hover was requested for; lets the painter ignore stale
    /// hovers after the user jumps to a different file.
    path: PathBuf,
    /// Rendered text content (Markdown stripped to plain runs for now).
    contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenameInput {
    language: Language,
    path: PathBuf,
    line: usize,
    col: usize,
    value: String,
}

/// Headless application state for the v0.1 cockpit shell.
pub struct AppModel {
    detection: ProjectDetection,
    /// Filesystem seam (M4.10). Production uses [`StdFileSystem`]; tests
    /// inject a [`cockpit_project::FakeFileSystem`] via [`AppModel::with_env`].
    fs: Box<dyn FileSystem>,
    /// Process-spawn seam (M4.10). Production uses [`StdProcessRunner`];
    /// tests inject a [`cockpit_project::FakeProcessRunner`].
    process: Box<dyn ProcessRunner>,
    browser: FileBrowser,
    layout: WorkspaceLayout,
    router: InputRouter,
    theme: Theme,
    status: String,
    document: Option<OpenDocument>,
    palette: Option<Palette>,
    palette_mode: PaletteMode,
    finder: Option<FuzzyFinder>,
    file_index: Option<Vec<String>>,
    terminals: HashMap<cockpit_mux::PaneId, LiveTerminal>,
    /// Native mux state (v0.7 M7.4). Pane ids key the live terminal map
    /// so split panes can own independent PTYs while the mux model stays
    /// headless.
    mux_session: MuxSession,
    mux_prefix: PrefixDispatcher,
    redraw: Option<RedrawHandle>,
    cache_path: Option<PathBuf>,
    /// Most recent key chords, oldest first (spec §18.13 key event inspector).
    key_log: VecDeque<String>,
    /// Most recent command ids, oldest first (spec §18.13 command log).
    command_log: VecDeque<String>,
    /// Lazy per-language LSP clients (spec §19 / M3.5). Spawned on first
    /// open of a relevant file; never started on app launch.
    lsp_clients: HashMap<Language, LspClient>,
    /// Languages whose servers have already received `initialize`/`initialized`.
    lsp_initialized: HashSet<Language>,
    /// Latest `publishDiagnostics` payload per file (spec §23 v0.4 / M4.1).
    diagnostics: HashMap<PathBuf, Vec<Diagnostic>>,
    /// In-flight LSP requests keyed by request id, so [`Response`]s route
    /// to the right model update (M4.2: definition / hover).
    lsp_pending: HashMap<i64, LspPending>,
    /// Latest hover result, if any. Shown in the mode line / status bar
    /// until cleared (M4.2).
    hover: Option<HoverInfo>,
    /// Inline rename input (M4.3a), modal over the editor while active.
    rename_input: Option<RenameInput>,
    /// Manual LSP completion popup (M4.3b).
    completion: Option<CompletionPopup>,
    /// Active yes/no modal prompt (M4.4). When `Some`, it captures keys
    /// the same way the rename input does.
    confirm: Option<ConfirmPrompt>,
    /// What to do if the active [`confirm`](Self::confirm) prompt is accepted.
    confirm_intent: Option<PromptIntent>,
    /// Per-session opt-in for format-on-save (M4.4). Initialised from
    /// `editor.format_on_save`; toggled by the palette command without
    /// touching the config file on disk.
    format_on_save: bool,
    /// Optional notebook view-model for the open document (v0.5 M5.3
    /// wire-up). Populated when [`open_document`] detects a Jupytext
    /// `.sql` / `.ggsql` source or any `.qmd` file; `None` otherwise.
    notebook: Option<Notebook>,
    /// Detected analytics project (v0.5 M5.6 wire-up). Built once during
    /// [`new`] when a `models/` directory is present and refreshed by
    /// `Models: Build All`.
    analytics: Option<AnalyticsProject>,
    /// Tracks a pending `g` in the editor pane so local `gd` can dispatch LSP
    /// definition while non-LSP Vim chords like `gg` still reach the Vim FSM.
    editor_pending_g: bool,
    /// Tracks `<leader>c` in the editor pane so `<leader>ca` can dispatch the
    /// quick-fix command without adding a parallel command path.
    editor_pending_leader: u8,
    /// Most recent computed layout — populated each `paint()` so mouse hit
    /// tests can ask which pane an event landed in (M4.7).
    last_layout: Option<ComputedLayout>,
    /// Logical width passed to the most recent layout compute (M4.7).
    last_view_width: f32,
    /// Logical height passed to the most recent layout compute (M4.7).
    last_view_height: f32,
    /// Active pane-border drag, if any (M4.7).
    drag: Option<DragState>,
    /// Scroll offset (logical rows from the top) for the editor pane (M4.7).
    editor_scroll: f32,
    exit: bool,
}

/// One in-progress pane-border drag (M4.7). Records which side is being
/// dragged and the cursor anchor / starting width so the layout update is
/// a pure delta from the drag origin.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DragState {
    LeftBorder {
        start_x: f32,
        start_width: u32,
    },
    RightBorder {
        start_x: f32,
        start_width: u32,
    },
    MuxDivider {
        axis: MuxDragAxis,
        pane: cockpit_mux::PaneId,
        last_x: f32,
        last_y: f32,
        extent: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum MuxDragAxis {
    Horizontal,
    Vertical,
}

impl AppModel {
    /// Build the model for a detected project and its loaded file tree.
    ///
    /// This is pure — call [`restore_cached_state`](Self::restore_cached_state)
    /// afterwards to load persisted per-project state from disk. The
    /// std-backed fs and process runner are used; for hermetic tests call
    /// [`AppModel::with_env`] to inject fakes (M4.10).
    pub fn new(detection: ProjectDetection, tree: FileTree) -> Result<Self, String> {
        Self::with_env(
            detection,
            tree,
            Box::new(StdFileSystem),
            Box::new(StdProcessRunner),
        )
    }

    /// Build the model with caller-supplied env seams (M4.10). Used by
    /// hermetic tests to scrub real filesystem / subprocess access from
    /// the model — the v0.5 SqlEngine work depends on this pattern being
    /// in place.
    pub fn with_env(
        detection: ProjectDetection,
        tree: FileTree,
        fs: Box<dyn FileSystem>,
        process: Box<dyn ProcessRunner>,
    ) -> Result<Self, String> {
        let router = InputRouter::from_global_keys(&GlobalKeys::default())
            .map_err(|err| format!("input router setup failed: {err:?}"))?;
        Ok(Self {
            fs,
            process,
            browser: FileBrowser::new(tree),
            layout: WorkspaceLayout::new(),
            router,
            theme: Theme::default(),
            status: "Ready — select a file and press Enter to open it.".to_string(),
            document: None,
            palette: None,
            palette_mode: PaletteMode::Commands,
            finder: None,
            file_index: None,
            terminals: HashMap::new(),
            mux_session: MuxSession::new(detection.display_name.clone()),
            mux_prefix: PrefixDispatcher::default(),
            redraw: None,
            cache_path: None,
            key_log: VecDeque::with_capacity(DEBUG_LOG_SIZE),
            command_log: VecDeque::with_capacity(DEBUG_LOG_SIZE),
            lsp_clients: HashMap::new(),
            lsp_initialized: HashSet::new(),
            diagnostics: HashMap::new(),
            lsp_pending: HashMap::new(),
            hover: None,
            rename_input: None,
            completion: None,
            confirm: None,
            confirm_intent: None,
            format_on_save: false,
            notebook: None,
            analytics: None,
            editor_pending_g: false,
            editor_pending_leader: 0,
            last_layout: None,
            last_view_width: 0.0,
            last_view_height: 0.0,
            drag: None,
            editor_scroll: 0.0,
            exit: false,
            detection,
        })
    }

    /// Resolve this project's cache file and restore persisted state from it
    /// (pane widths, last active file — spec §7). Touches disk; the binary
    /// calls it once after [`new`](Self::new).
    pub fn restore_cached_state(&mut self) {
        let Ok(path) = project_cache_path(&self.detection.root_path) else {
            return;
        };
        let cache = ProjectCache::load(&path).unwrap_or_default();
        self.cache_path = Some(path);
        self.apply_cache(cache);
    }

    /// Apply a loaded user [`cockpit_config::Config`] onto the model.
    /// Currently honours `editor.format_on_save` (M4.4) and the layout
    /// width preferences (`ui.left_width` / `ui.right_width`). Other
    /// fields parse but are still inert — they will land as their
    /// owning subsystems get wired up.
    pub fn apply_user_config(&mut self, config: &cockpit_config::Config) {
        self.format_on_save = config.editor.format_on_save;

        let mut prefs = self.layout.preferences().clone();
        prefs.left_width = config.ui.left_width.into();
        prefs.right_width = config.ui.right_width.into();
        self.layout.set_preferences(prefs);
    }

    /// Best-effort refresh of git status badges (spec §23 v0.3 / M3.4). Shells
    /// out to `git status --porcelain`; no-ops when `git` is missing or the
    /// project is not a git working tree.
    pub fn refresh_git_status(&mut self) {
        let statuses = cockpit_project::git_status(&self.detection.root_path);
        self.browser.set_git_statuses(statuses);
    }

    /// Restore persisted pane widths and reopen the last active file.
    /// Also rehydrates the fuzzy-finder index from the cache so the
    /// first `Ctrl+P` press is instant (M6.6); the index falls back to
    /// a real filesystem walk if the cached snapshot turns out to be
    /// stale.
    fn apply_cache(&mut self, cache: ProjectCache) {
        let mut prefs = self.layout.preferences().clone();
        if let Some(width) = cache.left_width {
            prefs.left_width = u32::from(width);
        }
        if let Some(width) = cache.right_width {
            prefs.right_width = u32::from(width);
        }
        self.layout.set_preferences(prefs);

        if !cache.file_index.is_empty() {
            self.file_index = Some(cache.file_index);
        }

        if let Some(active) = cache.active_file
            && active.is_file()
        {
            self.open_document(active);
        }
    }

    /// Snapshot the per-project state worth persisting across sessions.
    /// Includes the fuzzy-finder index so the next launch can offer
    /// instant Ctrl+P without re-walking the project tree (M6.6).
    fn build_cache(&self) -> ProjectCache {
        let active_file = self.document.as_ref().map(|doc| doc.path.clone());
        let prefs = self.layout.preferences();
        ProjectCache {
            open_files: active_file.clone().into_iter().collect(),
            active_file,
            left_width: Some(prefs.left_width as u16),
            right_width: Some(prefs.right_width as u16),
            file_index: self.file_index.clone().unwrap_or_default(),
            ..ProjectCache::default()
        }
    }

    /// Persist per-project state. Called by the harness as the app exits.
    pub fn on_shutdown(&mut self) {
        let Some(path) = self.cache_path.clone() else {
            return;
        };
        if let Err(err) = self.build_cache().store(&path) {
            tracing::warn!(error = %err, "failed to store project cache");
        }
    }

    /// Project display name.
    pub fn project_name(&self) -> &str {
        &self.detection.display_name
    }

    /// Store the handle used to wake the event loop from background threads.
    pub fn set_redraw_handle(&mut self, handle: RedrawHandle) {
        self.redraw = Some(handle);
    }

    /// True once the application has requested to quit.
    pub fn wants_exit(&self) -> bool {
        self.exit
    }

    /// Route one key chord into a state change.
    pub fn dispatch(&mut self, chord: KeyChord) {
        self.record_key_event(&chord);
        // The palette and fuzzy finder are modal: while open they consume
        // every key.
        if self.confirm.is_some() {
            self.handle_confirm_key(&chord);
            return;
        }
        if self.rename_input.is_some() {
            self.handle_rename_key(&chord);
            return;
        }
        if self.completion.is_some() {
            self.handle_completion_key(&chord);
            return;
        }
        if self.palette.is_some() {
            self.handle_palette_key(&chord);
            return;
        }
        if self.finder.is_some() {
            self.handle_finder_key(&chord);
            return;
        }
        let focused = self.layout.focused();
        if focused == PaneId::Terminal && self.handle_mux_prefix(&chord) {
            return;
        }
        match self.router.route(focused, chord) {
            RoutedInput::Command(id) => self.run_command(id.as_str()),
            RoutedInput::Unhandled(chord) | RoutedInput::TerminalPassthrough(chord) => {
                self.handle_local(focused, &chord)
            }
        }
    }

    // ---- M4.7 — Mouse input -----------------------------------------------

    /// Route a primary-button press at logical-pixel `position`. Routes:
    /// click in the file browser → focus + select (double-click activates),
    /// click in the editor or terminal pane → focus that pane, click on a
    /// pane border → start a resize drag.
    pub fn on_pointer_down(&mut self, button: MouseButton, position: PointerPosition) {
        if button != MouseButton::Left {
            return;
        }
        let Some(layout) = self.last_layout.clone() else {
            return;
        };

        // Pane-border resize takes priority over pane-focus hit tests.
        if let Some(drag) = self.detect_border_drag(&layout, position) {
            self.drag = Some(drag);
            return;
        }

        if let Some(rect) = layout.files
            && pane_contains(rect, position)
        {
            self.layout.focus(PaneId::Files);
            self.handle_files_click(rect, position);
            return;
        }
        if pane_contains(layout.editor, position) {
            self.layout.focus(PaneId::Editor);
            return;
        }
        if let Some(rect) = layout.terminal
            && pane_contains(rect, position)
        {
            self.layout.focus(PaneId::Terminal);
            self.select_mux_pane_at(rect, position);
            self.ensure_terminal();
        }
    }

    /// Release any in-progress drag and forget the last anchor (M4.7).
    pub fn on_pointer_up(&mut self, button: MouseButton, _position: PointerPosition) {
        if button == MouseButton::Left {
            self.drag = None;
        }
    }

    /// Continue an in-progress drag (M4.7). Mouse moves outside a drag are
    /// ignored — cockpit has no hover state today.
    pub fn on_pointer_move(&mut self, position: PointerPosition) {
        let Some(drag) = self.drag else {
            return;
        };
        match drag {
            DragState::LeftBorder {
                start_x,
                start_width,
            } => {
                let delta = position.x - start_x;
                let new_width = (start_width as f32 + delta).clamp(120.0, 800.0);
                self.layout.set_left_width(new_width as u32);
            }
            DragState::RightBorder {
                start_x,
                start_width,
            } => {
                let delta = start_x - position.x;
                let new_width = (start_width as f32 + delta).clamp(160.0, 1000.0);
                self.layout.set_right_width(new_width as u32);
            }
            DragState::MuxDivider {
                axis,
                pane,
                last_x,
                last_y,
                extent,
            } => {
                let delta = match axis {
                    MuxDragAxis::Horizontal => position.x - last_x,
                    MuxDragAxis::Vertical => position.y - last_y,
                };
                let ratio_delta = delta / extent.max(1.0);
                if ratio_delta.abs() > f32::EPSILON {
                    let _ = self.mux_session.select_pane(pane);
                    let _ = self.mux_session.resize_pane(-ratio_delta);
                }
                self.drag = Some(DragState::MuxDivider {
                    axis,
                    pane,
                    last_x: position.x,
                    last_y: position.y,
                    extent,
                });
            }
        }
    }

    /// Scroll wheel delta (M4.7). Positive `dy` scrolls content down — the
    /// viewport sees rows leave the top. The terminal pane forwards the
    /// scroll to the live PTY (Zellij owns scroll-back); the editor pane
    /// shifts its viewport offset.
    pub fn on_scroll(&mut self, position: PointerPosition, _dx: f32, dy: f32) {
        let Some(layout) = self.last_layout.clone() else {
            return;
        };
        if pane_contains(layout.editor, position) {
            // 1 line per 20px of wheel travel.
            self.editor_scroll = (self.editor_scroll - dy / 20.0).max(0.0);
            return;
        }
        if let Some(rect) = layout.terminal
            && pane_contains(rect, position)
            && let Some(terminal) = self.active_terminal_mut()
        {
            // Forward the wheel as a Zellij/termwiz scroll arrow sequence
            // by emitting CSI A/B per step. termwiz interprets these as
            // line-up/line-down inside its scroll-back.
            let steps = (dy.abs() / 20.0).ceil() as i32;
            let bytes: &[u8] = if dy > 0.0 { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..steps {
                let _ = terminal.send_input(bytes);
            }
        }
    }

    /// Pane-border drag detection (M4.7). A 6-pixel hit band around each
    /// visible side-pane border starts a [`DragState`].
    fn detect_border_drag(
        &self,
        layout: &ComputedLayout,
        position: PointerPosition,
    ) -> Option<DragState> {
        let band = 6.0_f32;
        if let Some(rect) = layout.files {
            let border_x = (rect.x + rect.width) as f32;
            if (position.x - border_x).abs() <= band
                && position.y >= TOP_BAR_H
                && position.y <= TOP_BAR_H + rect.height as f32
            {
                return Some(DragState::LeftBorder {
                    start_x: position.x,
                    start_width: rect.width,
                });
            }
        }
        if let Some(rect) = layout.terminal {
            if let Some(drag) = self.detect_mux_border_drag(rect, position) {
                return Some(drag);
            }
            let border_x = rect.x as f32;
            if (position.x - border_x).abs() <= band
                && position.y >= TOP_BAR_H
                && position.y <= TOP_BAR_H + rect.height as f32
            {
                return Some(DragState::RightBorder {
                    start_x: position.x,
                    start_width: rect.width,
                });
            }
        }
        None
    }

    fn detect_mux_border_drag(&self, rect: UiRect, position: PointerPosition) -> Option<DragState> {
        let band = 6.0_f32;
        let pane_rects = self.mux_pane_rects_for_terminal(rect);
        if pane_rects.len() < 2 {
            return None;
        }

        for (index, a) in pane_rects.iter().enumerate() {
            for b in pane_rects.iter().skip(index + 1) {
                let a_right = a.rect.x + a.rect.width;
                let b_right = b.rect.x + b.rect.width;
                let a_bottom = a.rect.y + a.rect.height;
                let b_bottom = b.rect.y + b.rect.height;
                let y0 = a.rect.y.max(b.rect.y) as f32;
                let y1 = a_bottom.min(b_bottom) as f32;
                let x0 = a.rect.x.max(b.rect.x) as f32;
                let x1 = a_right.min(b_right) as f32;

                for border_x in [a_right, b_right] {
                    let separates = (border_x <= b.rect.x || border_x <= a.rect.x)
                        && y1 > y0
                        && position.y >= y0
                        && position.y <= y1;
                    if separates && (position.x - border_x as f32).abs() <= band {
                        return Some(DragState::MuxDivider {
                            axis: MuxDragAxis::Horizontal,
                            pane: b.pane,
                            last_x: position.x,
                            last_y: position.y,
                            extent: rect.width.max(1) as f32,
                        });
                    }
                }

                for border_y in [a_bottom, b_bottom] {
                    let separates = (border_y <= b.rect.y || border_y <= a.rect.y)
                        && x1 > x0
                        && position.x >= x0
                        && position.x <= x1;
                    if separates && (position.y - border_y as f32).abs() <= band {
                        return Some(DragState::MuxDivider {
                            axis: MuxDragAxis::Vertical,
                            pane: b.pane,
                            last_x: position.x,
                            last_y: position.y,
                            extent: rect.height.max(1) as f32,
                        });
                    }
                }
            }
        }
        None
    }

    /// Translate a click in the files pane to a row index and act on it:
    /// single click selects, double-click is the same as Enter (open file
    /// or expand/collapse). For v0.4 cockpit treats the same click as both
    /// — a double-click counter would land here later if needed.
    fn handle_files_click(&mut self, rect: UiRect, position: PointerPosition) {
        let pane_top = rect.y as f32 + TOP_BAR_H + HEADER_H + PAD * 0.5;
        let local_y = position.y - pane_top;
        if local_y < 0.0 {
            return;
        }
        let index = (local_y / ROW_H) as usize;
        if index >= self.browser.rows().len() {
            return;
        }
        self.browser.select_row(index);
        self.activate_selection();
    }

    fn select_mux_pane_at(&mut self, rect: UiRect, position: PointerPosition) {
        let Some(pane) = self
            .mux_pane_rects_for_terminal(rect)
            .into_iter()
            .find(|pane| mux_rect_contains(pane.rect, position))
            .map(|pane| pane.pane)
        else {
            return;
        };
        if self.mux_session.select_pane(pane).is_ok() {
            self.status = format!("Mux: focused {pane}.");
        }
    }

    fn mux_pane_rects_for_terminal(&self, rect: UiRect) -> Vec<cockpit_mux::PaneRect> {
        self.mux_session.active_window().pane_rects(
            MuxRect::new(
                rect.x,
                rect.y + TOP_BAR_H as u32 + HEADER_H as u32,
                rect.width,
                rect.height.saturating_sub(HEADER_H as u32),
            ),
            1,
        )
    }

    /// Push a chord onto the recent-key ring buffer (spec §18.13).
    fn record_key_event(&mut self, chord: &KeyChord) {
        if self.key_log.len() == DEBUG_LOG_SIZE {
            self.key_log.pop_front();
        }
        self.key_log.push_back(chord.to_string());
    }

    /// Push a command id onto the recent-command ring buffer (spec §18.13).
    fn record_command(&mut self, id: &str) {
        if self.command_log.len() == DEBUG_LOG_SIZE {
            self.command_log.pop_front();
        }
        self.command_log.push_back(id.to_string());
    }

    /// Apply a resolved global command.
    fn run_command(&mut self, id: &str) {
        self.record_command(id);
        if self.run_mux_command(id) {
            return;
        }
        match id {
            command_ids::FOCUS_FILES => self.layout.focus(PaneId::Files),
            command_ids::FOCUS_EDITOR => self.layout.focus(PaneId::Editor),
            command_ids::FOCUS_TERMINAL => {
                self.layout.focus(PaneId::Terminal);
                self.ensure_terminal();
            }
            command_ids::TOGGLE_FILES => self.layout.toggle_files(),
            command_ids::TOGGLE_TERMINAL => self.layout.toggle_terminal(),
            command_ids::SAVE => self.save_document(),
            command_ids::COMMAND_PALETTE => self.open_palette(),
            command_ids::FUZZY_OPEN => self.open_finder(),
            MISE_RUN_TASK => self.open_mise_tasks(),
            TERMINAL_OPEN_PATH => self.open_terminal_paths(),
            TERMINAL_SEND_FILE_PATH => self.send_file_path_to_terminal(),
            TERMINAL_SEND_SELECTION => self.send_selection_to_terminal(),
            TEST_RUN_ALL => self.run_test_all(),
            TEST_RUN_CURRENT_FILE => self.run_test_current_file(),
            TEST_RUN_NEAREST => self.run_test_nearest(),
            DEBUG_SHOW_KEY_EVENTS => self.debug_show_key_events(),
            DEBUG_SHOW_COMMAND_LOG => self.debug_show_command_log(),
            DEBUG_SHOW_PANE_TREE => self.debug_show_pane_tree(),
            DEBUG_SHOW_PROJECT_STATE => self.debug_show_project_state(),
            DEBUG_RELOAD_CONFIG => self.debug_reload_config(),
            DEBUG_SHOW_STARTUP_TRACE => self.debug_show_startup_trace(),
            LSP_GOTO_DEFINITION => self.request_goto_definition(),
            LSP_SHOW_HOVER => self.request_show_hover(),
            LSP_RENAME => self.open_rename_input(),
            LSP_COMPLETION => self.request_completion(),
            LSP_CODE_ACTION => self.request_code_action(),
            EDITOR_FORMAT => self.request_format(FormatTrigger::Manual),
            EDITOR_TOGGLE_FORMAT_ON_SAVE => self.toggle_format_on_save(),
            NOTEBOOK_RUN_ACTIVE_CELL => self.run_active_notebook_cell(),
            NOTEBOOK_NEXT_CELL => self.notebook_next_cell(),
            NOTEBOOK_PREVIOUS_CELL => self.notebook_previous_cell(),
            NOTEBOOK_INSERT_CELL_BELOW => self.notebook_insert_cell_below(),
            MODELS_BUILD_ALL => self.run_build_all_models(),
            MODELS_SHOW_DAG => self.show_dag_summary(),
            QUARTO_RENDER => self.render_quarto_document(),
            APP_QUIT => self.exit = true,
            other => self.status = format!("Unhandled command `{other}`."),
        }
    }

    /// Open the command palette over the workspace.
    fn open_palette(&mut self) {
        self.palette_mode = PaletteMode::Commands;
        self.palette = Some(Palette::new(palette_entries()));
        self.status = "Command palette — type to filter, Enter to run, Esc to close.".to_string();
    }

    /// Open the mise-task picker, populated from the detected project tasks.
    fn open_mise_tasks(&mut self) {
        let tasks = &self.detection.mise.tasks;
        if tasks.is_empty() {
            self.status = "No mise tasks detected for this project.".to_string();
            return;
        }
        let entries = tasks
            .iter()
            .map(|task| {
                let title = match task.description.as_deref() {
                    Some(desc) if !desc.is_empty() => format!("{}  —  {desc}", task.name),
                    _ => task.name.clone(),
                };
                PaletteEntry::new(task.name.as_str(), title)
            })
            .collect();
        self.palette_mode = PaletteMode::MiseTasks;
        self.palette = Some(Palette::new(entries));
        self.status = "Run mise task — type to filter, Enter to run, Esc to close.".to_string();
    }

    fn run_mux_command(&mut self, id: &str) -> bool {
        match id {
            mux_command_ids::SPLIT_HORIZONTAL => self.mux_split(SplitDirection::Horizontal),
            mux_command_ids::SPLIT_VERTICAL => self.mux_split(SplitDirection::Vertical),
            mux_command_ids::KILL_PANE => self.mux_kill_pane(),
            mux_command_ids::NEXT_PANE
            | mux_command_ids::FOCUS_RIGHT
            | mux_command_ids::FOCUS_DOWN => self.mux_next_pane(),
            mux_command_ids::LAST_PANE
            | mux_command_ids::FOCUS_LEFT
            | mux_command_ids::FOCUS_UP => self.mux_previous_pane(),
            mux_command_ids::SWAP_PANE_NEXT => self.mux_swap_active_pane(),
            mux_command_ids::RESIZE_RIGHT | mux_command_ids::RESIZE_DOWN => {
                self.mux_resize_active_pane(0.05)
            }
            mux_command_ids::RESIZE_LEFT | mux_command_ids::RESIZE_UP => {
                self.mux_resize_active_pane(-0.05)
            }
            mux_command_ids::NEXT_WINDOW => self.mux_next_window(),
            mux_command_ids::PREVIOUS_WINDOW => self.mux_previous_window(),
            mux_command_ids::NEW_WINDOW => self.mux_new_window(),
            mux_command_ids::KILL_WINDOW => self.mux_kill_window(),
            mux_command_ids::NEXT_LAYOUT => self.mux_next_layout(),
            mux_command_ids::RENAME_WINDOW => {
                self.status = "Mux: rename window UI lands with the command palette input surface."
                    .to_string();
            }
            mux_command_ids::ZOOM_PANE => self.mux_toggle_zoom(),
            mux_command_ids::COPY_MODE => {
                self.status = "Mux: copy mode lands in M7.6.".to_string();
            }
            mux_command_ids::PASTE => {
                self.status = "Mux: paste uses copy-mode buffer in M7.6.".to_string();
            }
            mux_command_ids::DETACH => {
                self.status = "Mux: detach/attach lands in M7.5.".to_string();
            }
            other => {
                if let Some(index) = mux_select_window_index(other) {
                    self.mux_select_window(index);
                } else {
                    return false;
                }
            }
        }
        true
    }

    fn mux_split(&mut self, dir: SplitDirection) {
        let pane = self.mux_session.split_active(dir);
        self.layout.focus(PaneId::Terminal);
        self.status = format!("Mux: split terminal pane ({pane}).");
        if self.redraw.is_some() {
            self.ensure_terminal();
        }
    }

    fn mux_kill_pane(&mut self) {
        match self.mux_session.kill_pane() {
            Ok(pane) => {
                self.terminals.remove(&pane);
                self.status = format!("Mux: killed {pane}.");
            }
            Err(err) => self.status = format!("Mux: {err}."),
        }
    }

    fn mux_next_pane(&mut self) {
        self.mux_session.next_pane();
        let pane = self.mux_session.active_window().active;
        self.status = format!("Mux: focused {pane}.");
        if self.redraw.is_some() {
            self.ensure_terminal();
        }
    }

    fn mux_previous_pane(&mut self) {
        self.mux_session.previous_pane();
        let pane = self.mux_session.active_window().active;
        self.status = format!("Mux: focused {pane}.");
        if self.redraw.is_some() {
            self.ensure_terminal();
        }
    }

    fn mux_resize_active_pane(&mut self, delta: f32) {
        match self.mux_session.resize_pane(delta) {
            Ok(()) => {
                let pane = self.mux_session.active_window().active;
                self.status = format!("Mux: resized {pane}.");
            }
            Err(err) => self.status = format!("Mux: {err}."),
        }
    }

    fn mux_swap_active_pane(&mut self) {
        match self.mux_session.swap_panes() {
            Ok(()) => {
                let pane = self.mux_session.active_window().active;
                self.status = format!("Mux: swapped {pane}.");
            }
            Err(err) => self.status = format!("Mux: {err}."),
        }
    }

    fn mux_toggle_zoom(&mut self) {
        match self.mux_session.toggle_zoom() {
            Some(pane) => self.status = format!("Mux: zoomed {pane}."),
            None => self.status = "Mux: unzoomed pane.".to_string(),
        }
    }

    fn mux_new_window(&mut self) {
        let index = self.mux_session.windows.len();
        let id = self.mux_session.new_window(index.to_string());
        self.status = format!("Mux: new window {}.", id.get());
    }

    fn mux_kill_window(&mut self) {
        let panes = self.mux_session.active_window().layout.leaves();
        match self.mux_session.kill_window() {
            Ok(window) => {
                for pane in panes {
                    self.terminals.remove(&pane);
                }
                self.status = format!("Mux: killed window {}.", window.get());
                if self.redraw.is_some() {
                    self.ensure_terminal();
                }
            }
            Err(err) => self.status = format!("Mux: {err}."),
        }
    }

    fn mux_next_window(&mut self) {
        self.mux_session.next_window();
        self.status = format!("Mux: window {}.", self.mux_session.active.get());
        if self.redraw.is_some() && self.layout.focused() == PaneId::Terminal {
            self.ensure_terminal();
        }
    }

    fn mux_previous_window(&mut self) {
        self.mux_session.previous_window();
        self.status = format!("Mux: window {}.", self.mux_session.active.get());
        if self.redraw.is_some() && self.layout.focused() == PaneId::Terminal {
            self.ensure_terminal();
        }
    }

    fn mux_select_window(&mut self, index: usize) {
        match self.mux_session.select_window(index) {
            Ok(()) => {
                self.status = format!("Mux: window {}.", self.mux_session.active.get());
                if self.redraw.is_some() && self.layout.focused() == PaneId::Terminal {
                    self.ensure_terminal();
                }
            }
            Err(err) => self.status = format!("Mux: {err}."),
        }
    }

    fn mux_next_layout(&mut self) {
        let preset = self.mux_session.next_layout();
        self.status = format!("Mux: selected {preset:?} layout.");
    }

    /// Send `mise run <task>` to the terminal session, starting it if needed.
    fn run_mise_task(&mut self, task: &str) {
        self.ensure_terminal();
        let command = format!("mise run {task}\r");
        match self.active_terminal_mut() {
            Some(terminal) => match terminal.send_input(command.as_bytes()) {
                Ok(()) => {
                    self.layout.focus(PaneId::Terminal);
                    self.status = format!("Running mise task `{task}`.");
                }
                Err(err) => self.status = format!("Could not run `{task}`: {err}"),
            },
            None => self.status = format!("Cannot run `{task}` — terminal unavailable."),
        }
    }

    /// Paste the current file's path into the terminal prompt (spec §17). The
    /// path is project-relative when possible, matching the `path:line:col`
    /// form printed in terminal output.
    fn send_file_path_to_terminal(&mut self) {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No file open to send.".to_string();
            return;
        };
        let rendered = render_document_path(&doc.path, &self.detection.root_path);
        let bytes = paste_to_terminal(&rendered);
        let success = format!("Sent `{rendered}` to terminal.");
        self.paste_into_terminal(&bytes, success);
    }

    /// Paste the editor's visual-mode selection into the terminal prompt (spec
    /// §17). Reports a clear status when there is no document or no selection.
    fn send_selection_to_terminal(&mut self) {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No file open to send a selection from.".to_string();
            return;
        };
        let Some((start, end)) = doc.editor.selection() else {
            self.status = "No selection — enter Visual mode and select first.".to_string();
            return;
        };
        let text = doc.editor.buffer().slice(start..end);
        let bytes = paste_to_terminal(&text);
        let success = format!("Sent selection ({} bytes) to terminal.", text.len());
        self.paste_into_terminal(&bytes, success);
    }

    /// Paste `bytes` into the terminal, starting one on demand and focusing the
    /// terminal pane on success.
    fn paste_into_terminal(&mut self, bytes: &[u8], success: String) {
        self.ensure_terminal();
        match self.active_terminal_mut() {
            Some(terminal) => match terminal.send_input(bytes) {
                Ok(()) => {
                    self.layout.focus(PaneId::Terminal);
                    self.status = success;
                }
                Err(err) => self.status = format!("Terminal write failed: {err}"),
            },
            None => self.status = "Terminal unavailable.".to_string(),
        }
    }

    /// Run `mise run test` for the whole project (spec §16 Test: Run All).
    fn run_test_all(&mut self) {
        if !self.has_test_task() {
            self.status = TEST_TASK_MISSING.to_string();
            return;
        }
        self.send_command_to_terminal(
            &format!("mise run {TEST_TASK}"),
            "Running `mise run test`.".to_string(),
        );
    }

    /// Run `mise run test -- <file>` targeting the current document (spec §16
    /// Test: Run Current File). Whether the file path is honoured depends on
    /// the user's `test` task forwarding extra args (e.g. via `$@`).
    fn run_test_current_file(&mut self) {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No file open to test.".to_string();
            return;
        };
        if !self.has_test_task() {
            self.status = TEST_TASK_MISSING.to_string();
            return;
        }
        let path = render_document_path(&doc.path, &self.detection.root_path);
        self.send_command_to_terminal(
            &format!("mise run {TEST_TASK} -- {path}"),
            format!("Running tests for `{path}`."),
        );
    }

    /// Run `mise run test -- <name>` targeting the function declaration nearest
    /// to the cursor (spec §16 Test: Run Nearest).
    fn run_test_nearest(&mut self) {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No file open to test.".to_string();
            return;
        };
        if !self.has_test_task() {
            self.status = TEST_TASK_MISSING.to_string();
            return;
        }
        let language = Language::from_path(&doc.path);
        let cursor_byte = doc.editor.cursor().byte();
        let Some(name) = nearest_test_name(doc.editor.buffer(), cursor_byte, language) else {
            self.status = "No nearby test function found.".to_string();
            return;
        };
        self.send_command_to_terminal(
            &format!("mise run {TEST_TASK} -- {name}"),
            format!("Running test `{name}`."),
        );
    }

    /// True when the detected project defines a [`TEST_TASK`] mise task.
    fn has_test_task(&self) -> bool {
        self.detection
            .mise
            .tasks
            .iter()
            .any(|task| task.name == TEST_TASK)
    }

    /// Type `command\r` into the terminal — start one on demand, focus it on
    /// success, and report status either way. Used by the Test: * commands.
    fn send_command_to_terminal(&mut self, command: &str, success: String) {
        self.ensure_terminal();
        let line = format!("{command}\r");
        match self.active_terminal_mut() {
            Some(terminal) => match terminal.send_input(line.as_bytes()) {
                Ok(()) => {
                    self.layout.focus(PaneId::Terminal);
                    self.status = success;
                }
                Err(err) => self.status = format!("Terminal write failed: {err}"),
            },
            None => self.status = "Terminal unavailable.".to_string(),
        }
    }

    /// Surface the recent key-chord buffer in the status line and tracing log.
    fn debug_show_key_events(&mut self) {
        let recent = self.recent_keys_summary();
        tracing::info!(keys = %recent, "debug: show key events");
        self.status = format!("Key events: {recent}");
    }

    /// Surface the recent command-dispatch log in the status line and tracing.
    fn debug_show_command_log(&mut self) {
        let recent = self.recent_commands_summary();
        tracing::info!(commands = %recent, "debug: show command log");
        self.status = format!("Commands: {recent}");
    }

    /// Surface the current pane tree (focus, dimensions, terminal state).
    fn debug_show_pane_tree(&mut self) {
        let summary = self.pane_tree_summary();
        tracing::info!(panes = %summary, "debug: show pane tree");
        self.status = format!("Panes: {summary}");
    }

    /// Surface the project detection result: name, signals, mise contents.
    fn debug_show_project_state(&mut self) {
        let summary = self.project_state_summary();
        tracing::info!(project = %summary, "debug: show project state");
        self.status = format!("Project: {summary}");
    }

    /// Re-apply the default config to the input router. A real user-config
    /// load path lands later; for now this exercises the reload code path so
    /// keybinding changes from a future config edit can be wired through here.
    /// Surface the recorded startup trace in the status line (v0.6 M6.7).
    fn debug_show_startup_trace(&mut self) {
        let snapshot = crate::startup::snapshot();
        let text = crate::startup::format_snapshot(&snapshot);
        tracing::info!(startup = %text, "debug: show startup trace");
        self.status = text;
    }

    fn debug_reload_config(&mut self) {
        match InputRouter::from_global_keys(&GlobalKeys::default()) {
            Ok(router) => {
                self.router = router;
                tracing::info!("debug: reload config — defaults restored");
                self.status =
                    "Config reloaded (defaults — user-config wiring lands later).".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "debug: reload config failed");
                self.status = format!("Config reload failed: {err:?}");
            }
        }
    }

    fn recent_keys_summary(&self) -> String {
        if self.key_log.is_empty() {
            return "<none>".to_string();
        }
        self.key_log.iter().cloned().collect::<Vec<_>>().join(", ")
    }

    fn recent_commands_summary(&self) -> String {
        if self.command_log.is_empty() {
            return "<none>".to_string();
        }
        self.command_log
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn pane_tree_summary(&self) -> String {
        let prefs = self.layout.preferences();
        let focused = self.layout.focused();
        let term = self.terminals.len();
        let mux_window = self.mux_session.active_window();
        let mux_panes = mux_window.layout.leaves().len();
        format!(
            "files={}px, terminal={}px, focused={:?}, terminal_procs={term}, mux_window={}, mux_panes={}, mux_active={}",
            prefs.left_width,
            prefs.right_width,
            focused,
            mux_window.id.get(),
            mux_panes,
            mux_window.active,
        )
    }

    fn project_state_summary(&self) -> String {
        let signals = self
            .detection
            .signals
            .iter()
            .map(|signal| format!("{:?}", signal.kind))
            .collect::<Vec<_>>()
            .join("/");
        let mise = &self.detection.mise;
        let mise_state = if mise.detected {
            format!(
                "mise[{}tasks, {}tools, available={}]",
                mise.tasks.len(),
                mise.tools.len(),
                mise.available,
            )
        } else {
            "no mise".to_string()
        };
        format!(
            "name=`{}`, signals=[{}], {mise_state}",
            self.detection.display_name,
            if signals.is_empty() {
                "—".to_string()
            } else {
                signals
            },
        )
    }

    /// Scan the terminal's visible output for file references (spec §17). With
    /// a single match jump straight to it; with several, offer a picker.
    fn open_terminal_paths(&mut self) {
        let Some(terminal) = self.active_terminal() else {
            self.status = "No terminal — start one to navigate its output.".to_string();
            return;
        };
        let grid = terminal.snapshot().grid;
        let mut references: Vec<String> = Vec::new();
        for path_match in detect_paths_in_grid(&grid) {
            let reference = path_match.reference();
            if !references.contains(&reference) {
                references.push(reference);
            }
        }
        if references.is_empty() {
            self.status = "No file paths in the terminal output.".to_string();
        } else if references.len() == 1 {
            let reference = references.into_iter().next().unwrap_or_default();
            self.open_path_reference(&reference);
        } else {
            let entries = references
                .iter()
                .map(|reference| PaletteEntry::new(reference.as_str(), reference.clone()))
                .collect();
            self.palette_mode = PaletteMode::TerminalPaths;
            self.palette = Some(Palette::new(entries));
            self.status = "Open path — type to filter, Enter to jump, Esc to close.".to_string();
        }
    }

    /// Resolve a `path[:line[:col]]` reference against the project root and
    /// open it in the editor at that location.
    fn open_path_reference(&mut self, reference: &str) {
        let Some(path_match) = detect_paths(reference).into_iter().next() else {
            self.status = format!("Not a file reference: {reference}");
            return;
        };
        let path = self.detection.root_path.join(&path_match.path);
        if !path.is_file() {
            self.status = format!("File not found: {}", path_match.path);
            return;
        }
        self.open_document_at(path, path_match.line, path_match.column);
    }

    /// Open `path`, then place the cursor at a 1-based `line`/`column` when the
    /// reference carried one.
    fn open_document_at(&mut self, path: PathBuf, line: Option<u32>, column: Option<u32>) {
        self.open_document(path);
        let Some(line) = line else {
            return;
        };
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        let line0 = line.saturating_sub(1) as usize;
        let col0 = column.unwrap_or(1).saturating_sub(1) as usize;
        doc.editor.goto(line0, col0);
        let name = doc.name.clone();
        self.status = format!("Opened {name} at {}:{}", line0 + 1, col0 + 1);
    }

    /// Drive the modal command palette from one key chord.
    fn handle_palette_key(&mut self, chord: &KeyChord) {
        if self.palette.is_none() {
            return;
        }
        if is_chord(chord, "Escape") {
            self.palette = None;
            self.status = "Command palette closed.".to_string();
            return;
        }
        if is_chord(chord, "Enter") {
            let selected = self.palette.as_ref().and_then(Palette::activate);
            let mode = self.palette_mode;
            self.palette = None;
            match (selected, mode) {
                (Some(id), PaletteMode::Commands) => self.run_command(id.as_str()),
                (Some(id), PaletteMode::MiseTasks) => self.run_mise_task(id.as_str()),
                (Some(id), PaletteMode::TerminalPaths) => self.open_path_reference(id.as_str()),
                (None, _) => self.status = "Nothing selected.".to_string(),
            }
            return;
        }

        let Some(palette) = self.palette.as_mut() else {
            return;
        };
        if is_chord(chord, "ArrowDown") {
            palette.move_down();
        } else if is_chord(chord, "ArrowUp") {
            palette.move_up();
        } else if is_chord(chord, "Backspace") {
            palette.pop_char();
        } else if let Some(c) = chord_to_char(chord) {
            palette.push_char(c);
        }
    }

    /// Open the fuzzy file finder, indexing the project on first use.
    fn open_finder(&mut self) {
        if self.file_index.is_none() {
            match walk_project_files(&self.detection.root_path) {
                Ok(files) => {
                    let index = files
                        .iter()
                        .map(|path| path.to_string_lossy().replace('\\', "/"))
                        .collect();
                    self.file_index = Some(index);
                }
                Err(err) => {
                    self.status = format!("Could not index project files: {err}");
                    return;
                }
            }
        }
        let index = self.file_index.clone().unwrap_or_default();
        self.status = format!(
            "Fuzzy open — {} files; type to filter, Enter to open, Esc to close.",
            index.len()
        );
        self.finder = Some(FuzzyFinder::new(index));
    }

    /// Drive the modal fuzzy finder from one key chord.
    fn handle_finder_key(&mut self, chord: &KeyChord) {
        if self.finder.is_none() {
            return;
        }
        if is_chord(chord, "Escape") {
            self.finder = None;
            self.status = "Fuzzy open closed.".to_string();
            return;
        }
        if is_chord(chord, "Enter") {
            let selected = self
                .finder
                .as_ref()
                .and_then(|finder| finder.highlighted().map(str::to_string));
            self.finder = None;
            match selected {
                Some(relative) => {
                    let path = self.detection.root_path.join(relative);
                    self.open_document(path);
                }
                None => self.status = "No file selected.".to_string(),
            }
            return;
        }

        let Some(finder) = self.finder.as_mut() else {
            return;
        };
        if is_chord(chord, "ArrowDown") {
            finder.move_down();
        } else if is_chord(chord, "ArrowUp") {
            finder.move_up();
        } else if is_chord(chord, "Backspace") {
            finder.pop_char();
        } else if let Some(c) = chord_to_char(chord) {
            finder.push_char(c);
        }
    }

    /// Drive the inline rename prompt (M4.3a).
    fn handle_rename_key(&mut self, chord: &KeyChord) {
        if is_chord(chord, "Escape") {
            self.rename_input = None;
            self.status = "Rename cancelled.".to_string();
            return;
        }
        if is_chord(chord, "Enter") {
            self.submit_rename();
            return;
        }
        let Some(input) = self.rename_input.as_mut() else {
            return;
        };
        if is_chord(chord, "Backspace") {
            input.value.pop();
        } else if let Some(c) = chord_to_char(chord)
            && is_rename_input_char(c)
        {
            input.value.push(c);
        }
        self.status = format!("Rename: {}", input.value);
    }

    /// Drive the manual completion popup (M4.3b).
    fn handle_completion_key(&mut self, chord: &KeyChord) {
        if is_chord(chord, "Escape") {
            self.completion = None;
            self.status = "Completion closed.".to_string();
            return;
        }
        if is_chord(chord, "Enter") || is_chord(chord, "Tab") {
            self.accept_completion();
            return;
        }
        let Some(completion) = self.completion.as_mut() else {
            return;
        };
        if is_chord(chord, "ArrowDown") || is_chord(chord, "j") {
            completion.move_down();
        } else if is_chord(chord, "ArrowUp") || is_chord(chord, "k") {
            completion.move_up();
        }
    }

    /// Handle a chord no global binding claimed (pane-local shortcuts).
    fn handle_local(&mut self, focused: PaneId, chord: &KeyChord) {
        if *chord == KeyChord::single("q", Modifiers::CTRL) {
            self.exit = true;
            return;
        }
        match focused {
            PaneId::Files => self.handle_files_key(chord),
            PaneId::Editor => self.handle_editor_key(chord),
            PaneId::Terminal => self.handle_terminal_key(chord),
        }
    }

    /// Resolve the optional per-project Zellij layout file (spec §9 / §10 v0.3).
    /// Returns the absolute path on a successful KDL parse; surfaces a status
    /// warning and falls back to no-layout on read/parse errors so a broken
    /// layout never blocks the terminal from launching.
    fn resolve_zellij_layout(&mut self) -> Option<PathBuf> {
        let configured = self
            .detection
            .mise
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.zellij_layout.as_deref())?;
        let absolute = self.detection.root_path.join(configured);
        match ZellijLayout::load(&absolute) {
            Ok(layout) => Some(layout.path),
            Err(err) => {
                self.status = format!("Zellij layout {} ignored: {err}", configured.display(),);
                None
            }
        }
    }

    /// Spawn the active mux pane's terminal session on first use.
    fn ensure_terminal(&mut self) {
        let pane = self.mux_session.active_window().active;
        if self.terminals.contains_key(&pane) {
            return;
        }
        let Some(redraw) = self.redraw.clone() else {
            self.status = "Terminal unavailable — no redraw handle.".to_string();
            return;
        };
        let layout = self.resolve_zellij_layout();
        let plan = plan_launch(
            &self.detection.display_name,
            layout.as_deref(),
            &PathBinaryLookup,
            ShellProfile::host_default(),
        );
        let (command, label) = match plan {
            LaunchPlan::Zellij(command) => {
                let label = match layout.as_deref() {
                    Some(path) => format!("zellij ({})", path.display()),
                    None => "zellij".to_string(),
                };
                (command, label)
            }
            LaunchPlan::Fallback { command, reason } => (command, format!("shell — {reason:?}")),
        };
        let wake: WakeFn = Box::new(move || redraw.request());
        match LiveTerminal::spawn(&command, PtyDimensions::new(24, 80), wake) {
            Ok(terminal) => {
                self.terminals.insert(pane, terminal);
                self.status = format!("Terminal started for {pane} ({label}).");
            }
            Err(err) => self.status = format!("Terminal failed to start: {err}"),
        }
    }

    fn active_terminal_mut(&mut self) -> Option<&mut LiveTerminal> {
        let pane = self.mux_session.active_window().active;
        self.terminals.get_mut(&pane)
    }

    fn active_terminal(&self) -> Option<&LiveTerminal> {
        let pane = self.mux_session.active_window().active;
        self.terminals.get(&pane)
    }

    /// Forward a chord to the PTY when the terminal pane is focused.
    fn handle_terminal_key(&mut self, chord: &KeyChord) {
        if self.handle_mux_prefix(chord) {
            return;
        }

        let Some(bytes) = chord_to_terminal_bytes(chord) else {
            return;
        };
        let Some(terminal) = self.active_terminal_mut() else {
            return;
        };
        if let Err(err) = terminal.send_input(&bytes) {
            self.status = format!("Terminal write failed: {err}");
        }
    }

    fn handle_mux_prefix(&mut self, chord: &KeyChord) -> bool {
        if let Some(commands) = self.mux_prefix.handle_key(chord) {
            let ids: Vec<String> = commands
                .into_iter()
                .map(|command| command.id().to_string())
                .collect();
            for id in ids {
                self.run_command(&id);
            }
            return true;
        }
        false
    }

    /// Resize live terminals so each grid matches its mux pane.
    fn sync_terminal_size(&mut self, rect: UiRect) {
        for pane in self.mux_pane_rects_for_terminal(rect) {
            let Some(terminal) = self.terminals.get_mut(&pane.pane) else {
                continue;
            };
            let char_w = FONT * CHAR_W_RATIO;
            let inner_w = (pane.rect.width as f32 - 2.0 * PAD).max(char_w);
            let inner_h = (pane.rect.height as f32 - PAD).max(ROW_H);
            let cols = (inner_w / char_w) as u16;
            let rows = (inner_h / ROW_H) as u16;
            let _ = terminal.resize(PtyDimensions::new(rows, cols));
        }
    }

    /// File-browser navigation when the files pane is focused.
    fn handle_files_key(&mut self, chord: &KeyChord) {
        if is_chord(chord, "j") || is_chord(chord, "ArrowDown") {
            self.browser.move_down();
        } else if is_chord(chord, "k") || is_chord(chord, "ArrowUp") {
            self.browser.move_up();
        } else if is_chord(chord, "Enter") {
            self.activate_selection();
        }
    }

    /// Feed a chord into the Vim state machine when the editor is focused.
    fn handle_editor_key(&mut self, chord: &KeyChord) {
        if *chord == KeyChord::single("Space", Modifiers::CTRL) {
            self.run_command(LSP_COMPLETION);
            return;
        }
        let normal_mode = self
            .document
            .as_ref()
            .is_some_and(|doc| doc.editor.mode() == Mode::Normal);
        if normal_mode {
            if is_chord(chord, "K") {
                self.editor_pending_g = false;
                self.editor_pending_leader = 0;
                self.run_command(LSP_SHOW_HOVER);
                return;
            }
            if self.editor_pending_leader == 2 {
                self.editor_pending_leader = 0;
                if is_chord(chord, "a") {
                    self.editor_pending_g = false;
                    self.run_command(LSP_CODE_ACTION);
                    return;
                }
                if is_chord(chord, "r") {
                    self.editor_pending_g = false;
                    self.run_command(LSP_RENAME);
                    return;
                }
            }
            if self.editor_pending_leader == 1 {
                self.editor_pending_leader = 0;
                if is_chord(chord, "c") {
                    self.editor_pending_leader = 2;
                    self.editor_pending_g = false;
                    return;
                }
            }
            if is_chord(chord, "Space") {
                self.editor_pending_leader = 1;
                self.editor_pending_g = false;
                return;
            }
            let mut replayed_pending = false;
            if self.editor_pending_g {
                self.editor_pending_g = false;
                if is_chord(chord, "d") {
                    self.run_command(LSP_GOTO_DEFINITION);
                    return;
                }
                self.feed_vim_key(VimKey::Char('g'));
                replayed_pending = true;
            }
            if !replayed_pending && is_chord(chord, "g") {
                self.editor_pending_g = true;
                return;
            }
        } else {
            self.editor_pending_g = false;
            self.editor_pending_leader = 0;
        }
        let Some(key) = chord_to_vim_key(chord) else {
            return;
        };
        self.feed_vim_key(key);
    }

    /// Feed one already-normalised key into the Vim state machine.
    fn feed_vim_key(&mut self, key: VimKey) {
        let signal = match self.document.as_mut() {
            Some(doc) => doc.editor.handle_key(key),
            None => return,
        };
        match signal {
            EditorSignal::None => {}
            EditorSignal::Save => self.save_document(),
            EditorSignal::Quit => self.close_document(),
            EditorSignal::SaveQuit => {
                self.save_document();
                self.close_document();
            }
        }
    }

    /// Expand/collapse a directory, or open the selected file.
    fn activate_selection(&mut self) {
        match self.browser.activate() {
            Ok(FileBrowserAction::OpenFile(path)) => self.open_document(path),
            Ok(FileBrowserAction::Toggled | FileBrowserAction::Nothing) => {}
            Err(err) => self.status = format!("File tree error: {err}"),
        }
    }

    /// Load a file into the editor pane. Disk read goes through
    /// [`Self::fs`] so the path is testable with fakes (M4.10).
    fn open_document(&mut self, path: PathBuf) {
        match self.fs.read_to_string(&path) {
            Ok(content) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.notebook = recognise_notebook(&path, &content);
                let suffix = match &self.notebook {
                    Some(nb) => format!(" — notebook ({} cells)", nb.cells.len()),
                    None => String::new(),
                };
                self.status = format!("Opened {name}{suffix}");
                let mut editor = Editor::new(&content);
                editor.set_language(Language::from_path(&path));
                self.document = Some(OpenDocument { editor, path, name });
                self.layout.focus(PaneId::Editor);
                self.start_lsp_for_document();
            }
            Err(err) => self.status = format!("Could not open {}: {err}", path.display()),
        }
    }

    /// Lazily spawn (and `initialize` / `didOpen`) the LSP client for the
    /// document just opened. Spec §19 hard rules apply: never on launch, only
    /// when a relevant file opens, never blocking, never for huge files,
    /// servers launched via `mise exec` so we never bypass the project env.
    fn start_lsp_for_document(&mut self) {
        let (path, language, text) = {
            let Some(doc) = self.document.as_ref() else {
                return;
            };
            let Some(language) = Language::from_path(&doc.path) else {
                return;
            };
            let size = doc.editor.buffer().len_bytes();
            if size > LSP_MAX_BYTES {
                tracing::info!(
                    ?language,
                    size,
                    "LSP skipped: file exceeds size cap (spec §19)",
                );
                return;
            }
            (doc.path.clone(), language, doc.editor.buffer().text())
        };
        let Some(config) = ServerConfig::for_language(language) else {
            return;
        };

        if !self.lsp_clients.contains_key(&language) {
            let argv = lsp_launch_argv(&config);
            let (program, args) = argv
                .split_first()
                .expect("lsp_launch_argv always returns a non-empty argv");
            match LspClient::spawn(program, args, Some(&self.detection.root_path)) {
                Ok(client) => {
                    tracing::info!(?language, command = %config.command, "LSP client spawned");
                    self.lsp_clients.insert(language, client);
                }
                Err(err) => {
                    tracing::warn!(?err, ?language, "LSP spawn failed — continuing without it");
                    return;
                }
            }
        }
        let client = self
            .lsp_clients
            .get(&language)
            .expect("client inserted above");

        if !self.lsp_initialized.contains(&language) {
            let root_uri = file_uri(&self.detection.root_path);
            let init_params = serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {},
                "clientInfo": {
                    "name": "cockpit",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            });
            if let Err(err) = client.request("initialize", init_params) {
                tracing::warn!(?err, "LSP initialize request failed to queue");
                return;
            }
            if let Err(err) = client.notify("initialized", serde_json::json!({})) {
                tracing::warn!(?err, "LSP initialized notification failed to queue");
                return;
            }
            self.lsp_initialized.insert(language);
        }

        let didopen_params = serde_json::json!({
            "textDocument": {
                "uri": file_uri(&path),
                "languageId": config.language_id(),
                "version": 1,
                "text": text,
            }
        });
        if let Err(err) = client.notify("textDocument/didOpen", didopen_params) {
            tracing::warn!(?err, "LSP textDocument/didOpen failed to queue");
        }
    }

    /// Write the open document to its file. When `format_on_save` is enabled
    /// the formatter runs immediately after the on-disk write succeeds (M4.4).
    /// Disk write goes through [`Self::fs`] (M4.10).
    fn save_document(&mut self) {
        let Some(doc) = self.document.as_mut() else {
            self.status = "No document to save.".to_string();
            return;
        };
        match self.fs.write(&doc.path, doc.editor.text().as_bytes()) {
            Ok(()) => {
                doc.editor.mark_saved();
                self.status = format!("Saved {}", doc.name);
            }
            Err(err) => {
                self.status = format!("Save failed: {err}");
                return;
            }
        }
        if self.format_on_save {
            self.request_format(FormatTrigger::Save);
        }
    }

    /// Close the open document, returning the editor pane to the welcome view.
    fn close_document(&mut self) {
        self.document = None;
        self.status = "Document closed.".to_string();
    }

    /// Drain every queued LSP message and apply it to model state. Cheap when
    /// quiescent (a few `try_recv`s) so it is safe to call once per frame.
    fn drain_lsp_messages(&mut self) {
        let mut messages: Vec<RecvMessage> = Vec::new();
        for client in self.lsp_clients.values() {
            while let Some(message) = client.try_recv() {
                messages.push(message);
            }
        }
        for message in messages {
            self.handle_lsp_message(message);
        }
    }

    /// Dispatch one inbound LSP message into the right place in the model.
    fn handle_lsp_message(&mut self, message: RecvMessage) {
        match message {
            RecvMessage::ServerNotification { method, params }
                if method == "textDocument/publishDiagnostics" =>
            {
                match serde_json::from_value::<PublishDiagnosticsParams>(params) {
                    Ok(parsed) => self.apply_publish_diagnostics(parsed),
                    Err(err) => {
                        tracing::warn!(?err, "publishDiagnostics: failed to parse params");
                    }
                }
            }
            RecvMessage::ServerNotification { method, .. } => {
                tracing::trace!(method = %method, "LSP notification");
            }
            RecvMessage::Response(response) => {
                // Route responses to whichever request kicked them off
                // (M4.2). Unmatched responses fall through to the existing
                // diagnostic logging.
                if let Some(id) = response.id
                    && let Some(pending) = self.lsp_pending.remove(&id)
                {
                    self.apply_lsp_response(pending, response);
                    return;
                }
                if let Some(error) = &response.error {
                    tracing::warn!(?response.id, code = error.code, message = %error.message, "LSP error response");
                } else {
                    tracing::trace!(?response.id, "LSP response");
                }
            }
            RecvMessage::ServerRequest { id, method, .. } => {
                tracing::debug!(id, method = %method, "LSP server request (ignored — M4.x will route)");
            }
            RecvMessage::Decode { error, .. } => {
                tracing::warn!(error = %error, "LSP decode failed");
            }
        }
    }

    /// Replace the stored diagnostics for the URI in `params`. An empty list
    /// clears the slot so badges disappear once the server clears them.
    fn apply_publish_diagnostics(&mut self, params: PublishDiagnosticsParams) {
        let Some(path) = path_from_file_uri(params.uri.as_str()) else {
            tracing::warn!(uri = %params.uri.as_str(), "publishDiagnostics: URI is not a file://");
            return;
        };
        if params.diagnostics.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            self.diagnostics.insert(path, params.diagnostics);
        }
    }

    /// Diagnostics currently attached to `path`, in document order.
    fn diagnostics_for(&self, path: &Path) -> &[Diagnostic] {
        self.diagnostics.get(path).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Issue a `textDocument/definition` request for the symbol under the
    /// cursor in the open document (M4.2). The response is matched back to
    /// this request via [`AppModel::lsp_pending`] and handled by
    /// [`apply_goto_definition_result`].
    fn request_goto_definition(&mut self) {
        let Some((language, path, line, col)) = self.cursor_for_lsp_request() else {
            return;
        };
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "position": { "line": line as u64, "character": col as u64 },
        });
        match client.request("textDocument/definition", params) {
            Ok(id) => {
                self.lsp_pending.insert(id, LspPending::GotoDefinition);
                self.status = "Looking up definition…".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP definition request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Issue a `textDocument/hover` request for the symbol under the cursor
    /// (M4.2). The response is handled by [`apply_hover_result`].
    fn request_show_hover(&mut self) {
        let Some((language, path, line, col)) = self.cursor_for_lsp_request() else {
            return;
        };
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "position": { "line": line as u64, "character": col as u64 },
        });
        match client.request("textDocument/hover", params) {
            Ok(id) => {
                self.lsp_pending
                    .insert(id, LspPending::Hover { path: path.clone() });
                self.status = "Looking up hover…".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP hover request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Start inline rename input seeded from the identifier under the cursor.
    fn open_rename_input(&mut self) {
        let Some((language, path, line, col)) = self.cursor_for_lsp_request() else {
            return;
        };
        let Some(doc) = self.document.as_ref() else {
            self.status = "No document open.".to_string();
            return;
        };
        let seed = symbol_under_cursor(&doc.editor.text(), doc.editor.cursor().byte())
            .unwrap_or_else(|| "new_name".to_string());
        self.rename_input = Some(RenameInput {
            language,
            path,
            line,
            col,
            value: seed.clone(),
        });
        self.status = format!("Rename: {seed}");
    }

    /// Submit the inline rename value through `prepareRename` first.
    fn submit_rename(&mut self) {
        let Some(input) = self.rename_input.take() else {
            return;
        };
        if input.value.trim().is_empty() {
            self.status = "Rename requires a non-empty name.".to_string();
            return;
        }
        let Some(client) = self.lsp_clients.get(&input.language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&input.path) },
            "position": { "line": input.line as u64, "character": input.col as u64 },
        });
        match client.request("textDocument/prepareRename", params) {
            Ok(id) => {
                self.lsp_pending.insert(
                    id,
                    LspPending::PrepareRename {
                        language: input.language,
                        path: input.path,
                        line: input.line,
                        col: input.col,
                        new_name: input.value,
                    },
                );
                self.status = "Preparing rename...".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP prepareRename request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Request manual LSP completions at the cursor (M4.3b).
    fn request_completion(&mut self) {
        let Some((language, path, line, col)) = self.cursor_for_lsp_request() else {
            return;
        };
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "position": { "line": line as u64, "character": col as u64 },
            "context": { "triggerKind": 1 },
        });
        match client.request("textDocument/completion", params) {
            Ok(id) => {
                self.lsp_pending.insert(id, LspPending::Completion);
                self.status = "Looking up completions...".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP completion request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Request code actions for the diagnostic nearest the cursor (M4.5).
    fn request_code_action(&mut self) {
        let Some((language, path, line, col)) = self.cursor_for_lsp_request() else {
            return;
        };
        let Some(diagnostic) = self.current_diagnostic(&path, line).cloned() else {
            self.status = "No diagnostic at the cursor.".to_string();
            return;
        };
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "range": {
                "start": { "line": diagnostic.range.start.line, "character": diagnostic.range.start.character },
                "end": { "line": diagnostic.range.end.line, "character": diagnostic.range.end.character },
            },
            "context": {
                "diagnostics": [diagnostic],
                "only": ["quickfix"],
            },
            "position": { "line": line as u64, "character": col as u64 },
        });
        match client.request("textDocument/codeAction", params) {
            Ok(id) => {
                self.lsp_pending.insert(id, LspPending::CodeAction);
                self.status = "Looking up code actions...".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP codeAction request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Common preflight for the two M4.2 request commands: returns the
    /// `(language, path, line, col)` needed to build a `TextDocumentPositionParams`
    /// payload, or sets `status` and yields `None` when the request cannot
    /// be made.
    fn cursor_for_lsp_request(&mut self) -> Option<(Language, PathBuf, usize, usize)> {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No document open.".to_string();
            return None;
        };
        let Some(language) = Language::from_path(&doc.path) else {
            self.status = "Unsupported language for LSP request.".to_string();
            return None;
        };
        let (line, col) = doc.editor.cursor().line_col(doc.editor.buffer());
        Some((language, doc.path.clone(), line, col))
    }

    fn current_diagnostic(&self, path: &Path, line: usize) -> Option<&Diagnostic> {
        let line = line as u32;
        self.diagnostics_for(path)
            .iter()
            .find(|diagnostic| {
                diagnostic.range.start.line <= line && diagnostic.range.end.line >= line
            })
            .or_else(|| self.diagnostics_for(path).first())
    }

    /// Apply a `Response` that matched a pending [`LspPending`] entry. Splits
    /// out so the dispatch in [`handle_lsp_message`] stays small.
    fn apply_lsp_response(&mut self, pending: LspPending, response: Response) {
        if let Some(error) = &response.error {
            self.status = format!("LSP error: {}", error.message);
            return;
        }
        let result = response.result.unwrap_or(serde_json::Value::Null);
        match pending {
            LspPending::GotoDefinition => self.apply_goto_definition_result(result),
            LspPending::Hover { path } => self.apply_hover_result(path, result),
            LspPending::CodeAction => self.apply_code_action_result(result),
            LspPending::PrepareRename {
                language,
                path,
                line,
                col,
                new_name,
            } => self.apply_prepare_rename_result(language, path, line, col, new_name, result),
            LspPending::Rename => self.apply_rename_result(result),
            LspPending::Completion => self.apply_completion_result(result),
            LspPending::Formatting { path, trigger } => {
                self.apply_formatting_result(path, trigger, result)
            }
        }
    }

    /// Open the file at the first usable Location/LocationLink returned by
    /// `textDocument/definition`, or surface "no definition" otherwise.
    fn apply_goto_definition_result(&mut self, result: serde_json::Value) {
        let Some((target_uri, line, character)) = parse_first_location(&result) else {
            self.status = "No definition found.".to_string();
            return;
        };
        let Some(path) = path_from_file_uri(&target_uri) else {
            self.status = format!("Definition URI was not a file://: {target_uri}");
            return;
        };
        // LSP positions are 0-based; `open_document_at` expects 1-based.
        self.open_document_at(path, Some(line + 1), Some(character + 1));
    }

    /// Store the rendered hover text (if any) for the requesting document
    /// and reflect it in the status bar.
    fn apply_hover_result(&mut self, from_path: PathBuf, result: serde_json::Value) {
        let Some(contents) = extract_hover_contents(&result) else {
            self.hover = None;
            self.status = "No hover information.".to_string();
            return;
        };
        let first_line = contents.lines().next().unwrap_or("").to_string();
        self.hover = Some(HoverInfo {
            path: from_path,
            contents,
        });
        self.status = if first_line.is_empty() {
            "Hover received.".to_string()
        } else {
            format!("Hover: {first_line}")
        };
    }

    /// Apply the first LSP code action that carries a workspace edit.
    fn apply_code_action_result(&mut self, result: serde_json::Value) {
        let Some(actions) = result.as_array() else {
            self.status = "No code actions available.".to_string();
            return;
        };
        for action in actions {
            let title = action
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("code action");
            let Some(edit) = action.get("edit") else {
                continue;
            };
            match self.apply_workspace_edit(edit) {
                Ok(count) if count > 0 => {
                    self.status = format!("Applied code action `{title}` ({count} edit(s)).");
                    return;
                }
                Ok(_) => {}
                Err(err) => {
                    self.status = format!("Code action failed: {err}");
                    return;
                }
            }
        }
        self.status = "No code action edit available.".to_string();
    }

    /// After `prepareRename` succeeds, issue the actual rename request.
    fn apply_prepare_rename_result(
        &mut self,
        language: Language,
        path: PathBuf,
        line: usize,
        col: usize,
        new_name: String,
        result: serde_json::Value,
    ) {
        if result.is_null() {
            self.status = "Rename is not available at the cursor.".to_string();
            return;
        }
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status = "Language server is not running for this file.".to_string();
            return;
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "position": { "line": line as u64, "character": col as u64 },
            "newName": new_name,
        });
        match client.request("textDocument/rename", params) {
            Ok(id) => {
                self.lsp_pending.insert(id, LspPending::Rename);
                self.status = "Renaming symbol...".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP rename request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Apply a `textDocument/rename` workspace edit.
    fn apply_rename_result(&mut self, result: serde_json::Value) {
        if result.is_null() {
            self.status = "Rename produced no edits.".to_string();
            return;
        }
        match self.apply_workspace_edit(&result) {
            Ok(count) if count > 0 => {
                self.status = format!("Rename applied ({count} edit(s)).");
            }
            Ok(_) => self.status = "Rename produced no edits.".to_string(),
            Err(err) => self.status = format!("Rename failed: {err}"),
        }
    }

    /// Store returned completion candidates in the popup view-model.
    fn apply_completion_result(&mut self, result: serde_json::Value) {
        let items_value = result
            .get("items")
            .and_then(serde_json::Value::as_array)
            .or_else(|| result.as_array());
        let Some(items_value) = items_value else {
            self.completion = None;
            self.status = "No completions available.".to_string();
            return;
        };
        let items: Vec<CompletionItem> = items_value
            .iter()
            .filter_map(parse_completion_item)
            .take(50)
            .collect();
        if items.is_empty() {
            self.completion = None;
            self.status = "No completions available.".to_string();
            return;
        }
        let first = items[0].label.clone();
        let count = items.len();
        self.completion = Some(CompletionPopup::new(items));
        self.status = format!("Completions: {count} candidate(s), first `{first}`.");
    }

    /// Format the open document — M4.4. Mise task wins; otherwise prompt to
    /// add one when a known formatter is detectable; otherwise fall back to
    /// LSP `textDocument/formatting`.
    fn request_format(&mut self, trigger: FormatTrigger) {
        let Some((path, language)) = self.document.as_ref().map(|doc| {
            let path = doc.path.clone();
            let language = Language::from_path(&path);
            (path, language)
        }) else {
            self.status = "No document to format.".to_string();
            return;
        };
        let language_id = language.and_then(|language| {
            ServerConfig::for_language(language).map(|config| config.language_id().to_string())
        });
        let plan = plan_format(
            &self.detection.mise,
            language_id.as_deref(),
            &FormatPathLookup,
        );
        match plan {
            FormatPlan::MiseTask { name } => self.run_format_mise_task(&path, &name, trigger),
            FormatPlan::SuggestMiseTask {
                formatter,
                suggested_run,
                from_mise_tools,
            } => self.prompt_add_format_task(formatter, suggested_run, from_mise_tools, trigger),
            FormatPlan::LspOnly => self.request_lsp_formatting(language, path, trigger),
        }
    }

    /// Run an existing `format` (or `format:<lang>`) mise task against the
    /// open document. The on-disk version is the source of truth: the buffer
    /// is already saved by the time we get here (either via [`save_document`]
    /// or via an explicit save flushed by the manual command), so we spawn
    /// `mise run <task> -- <path>` through [`Self::process`], wait for it,
    /// and reload the buffer through [`Self::fs`] (M4.10).
    fn run_format_mise_task(&mut self, path: &Path, task: &str, trigger: FormatTrigger) {
        // Manual triggers also need the on-disk file to match the buffer
        // before we hand it to the formatter — flush first.
        if trigger == FormatTrigger::Manual
            && let Some(doc) = self.document.as_ref()
            && doc.editor.is_dirty()
            && let Err(err) = self.fs.write(&doc.path, doc.editor.text().as_bytes())
        {
            self.status = format!("Format: pre-save failed: {err}");
            return;
        }

        let spec = ProcessSpec::new("mise")
            .arg("run")
            .arg(task)
            .arg("--")
            .arg(path.as_os_str())
            .current_dir(&self.detection.root_path);
        let output = match self.process.run(&spec) {
            Ok(output) => output,
            Err(err) => {
                self.status = format!("Format: could not run `mise run {task}`: {err}");
                return;
            }
        };
        if !output.success {
            let snippet = output
                .stderr_string()
                .lines()
                .next()
                .unwrap_or("(no output)")
                .to_string();
            self.status = format!("Format: `mise run {task}` failed: {snippet}");
            return;
        }
        if let Err(err) = self.reload_buffer_from_disk(path) {
            self.status = format!("Format: reload after `{task}` failed: {err}");
            return;
        }
        self.status = match trigger {
            FormatTrigger::Manual => format!("Formatted via `mise run {task}`."),
            FormatTrigger::Save => format!("Saved and formatted via `mise run {task}`."),
        };
    }

    /// Surface the M4.4 "Add `format` task to `mise.toml`?" confirmation
    /// modal. Stores the snippet to be written on confirm and the original
    /// trigger so the format retries after the user says yes.
    fn prompt_add_format_task(
        &mut self,
        formatter: KnownFormatter,
        suggested_run: String,
        from_mise_tools: bool,
        trigger: FormatTrigger,
    ) {
        let where_from = if from_mise_tools {
            "mise.toml [tools]"
        } else {
            "$PATH"
        };
        let title = format!(
            "Add `format` task to mise.toml? ({} found on {where_from})",
            formatter.binary()
        );
        let body = format!(
            "Cockpit will append:\n\n[tasks.format]\nrun = \"{}\"\n\nNothing is written until you confirm.",
            suggested_run.replace('"', "\\\"")
        );
        let snippet = render_format_task_snippet(&suggested_run);
        self.confirm = Some(ConfirmPrompt::new(title, body));
        self.confirm_intent = Some(PromptIntent::AddFormatTask {
            snippet,
            retry_trigger: trigger,
        });
        self.status =
            "Add format task? y/Enter to confirm, n/Escape to cancel, Tab to toggle.".to_string();
    }

    /// Issue a `textDocument/formatting` request against the open document's
    /// language server. The response is applied in [`apply_formatting_result`].
    fn request_lsp_formatting(
        &mut self,
        language: Option<Language>,
        path: PathBuf,
        trigger: FormatTrigger,
    ) {
        let Some(language) = language else {
            self.status = "No formatter and unsupported language for LSP fallback.".to_string();
            return;
        };
        let Some(client) = self.lsp_clients.get(&language) else {
            self.status =
                "No formatter detected and the language server is not running.".to_string();
            return;
        };
        let tab_size = 4_u64;
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri(&path) },
            "options": {
                "tabSize": tab_size,
                "insertSpaces": true,
                "trimTrailingWhitespace": true,
                "insertFinalNewline": true,
            }
        });
        match client.request("textDocument/formatting", params) {
            Ok(id) => {
                self.lsp_pending
                    .insert(id, LspPending::Formatting { path, trigger });
                self.status = "Requested formatting from language server...".to_string();
            }
            Err(err) => {
                tracing::warn!(?err, "LSP formatting request failed to queue");
                self.status = format!("LSP request failed: {err}");
            }
        }
    }

    /// Apply edits returned by `textDocument/formatting`. When the trigger
    /// was a save, the buffer is also flushed back to disk so the on-disk
    /// file ends up formatted (the buffer-only edit would otherwise stay
    /// out of sync with disk until the next save).
    fn apply_formatting_result(
        &mut self,
        path: PathBuf,
        trigger: FormatTrigger,
        result: serde_json::Value,
    ) {
        let Some(items) = result.as_array() else {
            if result.is_null() {
                self.status = "Formatter returned no edits.".to_string();
            } else {
                self.status = "Formatter result was not an edit array.".to_string();
            }
            return;
        };
        let mut by_path: HashMap<PathBuf, Vec<LspTextEdit>> = HashMap::new();
        if let Err(err) = collect_lsp_text_edits(
            &serde_json::Value::Array(items.clone()),
            &mut by_path,
            path.clone(),
        ) {
            self.status = format!("Formatter edits malformed: {err}");
            return;
        }
        let Some(edits) = by_path.remove(&path) else {
            self.status = "Formatter returned no edits.".to_string();
            return;
        };
        let mut sorted = edits;
        sorted.sort_by(|a, b| {
            b.start_line
                .cmp(&a.start_line)
                .then_with(|| b.start_character.cmp(&a.start_character))
        });
        let Some(doc) = self.document.as_mut() else {
            self.status = "Format: no document open.".to_string();
            return;
        };
        if doc.path != path {
            self.status = "Format: result is for a closed file.".to_string();
            return;
        }
        let result = apply_lsp_text_edits_to_editor(&mut doc.editor, &sorted);
        let (text_to_persist, count) = match result {
            Ok(count) => (doc.editor.text(), count),
            Err(err) => {
                self.status = format!("Format: apply failed: {err}");
                return;
            }
        };
        if trigger == FormatTrigger::Save {
            // Re-flush the now-formatted buffer through the env seam (M4.10).
            match self.fs.write(&path, text_to_persist.as_bytes()) {
                Ok(()) => {
                    if let Some(doc) = self.document.as_mut() {
                        doc.editor.mark_saved();
                    }
                    self.status = format!("Saved and formatted via LSP ({count} edit(s)).");
                }
                Err(err) => {
                    self.status = format!("Format applied but re-save failed: {err}");
                }
            }
        } else {
            self.status = format!("Formatted via LSP ({count} edit(s)).");
        }
    }

    /// Replace the open document's buffer with the on-disk contents of
    /// `path`. Used after an out-of-process formatter rewrites the file.
    /// Disk read goes through [`Self::fs`] (M4.10).
    fn reload_buffer_from_disk(&mut self, path: &Path) -> Result<(), String> {
        let text = self
            .fs
            .read_to_string(path)
            .map_err(|err| format!("read {}: {err}", path.display()))?;
        let Some(doc) = self.document.as_mut() else {
            return Err("no document open".to_string());
        };
        if doc.path != path {
            return Err("open document changed during format".to_string());
        }
        doc.editor.replace_all(&text);
        doc.editor.mark_saved();
        Ok(())
    }

    /// Apply the pending [`PromptIntent`] when the user accepts the modal.
    fn accept_confirm(&mut self) {
        let Some(intent) = self.confirm_intent.take() else {
            self.confirm = None;
            return;
        };
        self.confirm = None;
        match intent {
            PromptIntent::AddFormatTask {
                snippet,
                retry_trigger,
            } => self.add_format_task_and_retry(&snippet, retry_trigger),
        }
    }

    /// Append the `[tasks.format]` snippet to `mise.toml`, refresh the
    /// detected project, and retry the original format request so the user
    /// sees the formatter run immediately after confirming. All disk I/O
    /// goes through [`Self::fs`] (M4.10).
    fn add_format_task_and_retry(&mut self, snippet: &str, trigger: FormatTrigger) {
        let mise_path = self.detection.root_path.join("mise.toml");
        let updated = match self.fs.read_to_string(&mise_path) {
            Ok(existing) => {
                let mut updated = existing;
                if !updated.ends_with('\n') {
                    updated.push('\n');
                }
                updated.push_str(snippet);
                updated
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => snippet.to_string(),
            Err(err) => {
                self.status = format!("Could not read mise.toml: {err}");
                return;
            }
        };
        if let Err(err) = self.fs.write(&mise_path, updated.as_bytes()) {
            self.status = format!("Could not write mise.toml: {err}");
            return;
        }
        match cockpit_project::detect_mise_project_with(
            &self.detection.root_path,
            self.fs.as_ref(),
            self.process.as_ref(),
        ) {
            Ok(mise) => {
                self.detection.mise = mise;
                self.status = "Added [tasks.format] to mise.toml.".to_string();
            }
            Err(err) => {
                self.status = format!("mise.toml updated but reparse failed: {err}");
                return;
            }
        }
        self.request_format(trigger);
    }

    /// Flip the per-session format-on-save preference (M4.4).
    fn toggle_format_on_save(&mut self) {
        self.format_on_save = !self.format_on_save;
        self.status = if self.format_on_save {
            "Format on save: ON.".to_string()
        } else {
            "Format on save: OFF.".to_string()
        };
    }

    // ---- v0.5 notebook + analytics wire-up ------------------------------

    /// Execute the active notebook cell through DuckDB or ggsql, route
    /// the result back into [`Cell::apply_result`] (M5.3 wire-up).
    fn run_active_notebook_cell(&mut self) {
        let root = self.detection.root_path.clone();
        let Some(notebook) = self.notebook.as_mut() else {
            self.status = "No notebook open.".to_string();
            return;
        };
        let Some(cell) = notebook.active_cell_mut() else {
            self.status = "No active cell.".to_string();
            return;
        };
        if !cell.kind.executable() {
            self.status = format!("Cell {} is not executable.", notebook.active);
            return;
        }
        let source = cell.source.clone();
        let kind = cell.kind;
        cell.mark_running();
        let active = notebook.active;
        let result = run_cell_against_engines(&root, kind, &source);
        let Some(cell) = notebook.active_cell_mut() else {
            return;
        };
        cell.apply_result(result);
        let summary = match &cell.result {
            Some(cockpit_notebook::CellResult::Ok(query)) => {
                format!("Cell {active} ran: {} row(s).", query.rows.len())
            }
            Some(cockpit_notebook::CellResult::Err { message }) => {
                format!("Cell {active} failed: {message}")
            }
            None => format!("Cell {active}: no result."),
        };
        self.status = summary;
    }

    fn notebook_next_cell(&mut self) {
        let Some(notebook) = self.notebook.as_mut() else {
            self.status = "No notebook open.".to_string();
            return;
        };
        notebook.move_down();
        self.status = format!(
            "Notebook cell {} of {}.",
            notebook.active + 1,
            notebook.cells.len()
        );
    }

    fn notebook_previous_cell(&mut self) {
        let Some(notebook) = self.notebook.as_mut() else {
            self.status = "No notebook open.".to_string();
            return;
        };
        notebook.move_up();
        self.status = format!(
            "Notebook cell {} of {}.",
            notebook.active + 1,
            notebook.cells.len()
        );
    }

    fn notebook_insert_cell_below(&mut self) {
        let Some(notebook) = self.notebook.as_mut() else {
            self.status = "No notebook open.".to_string();
            return;
        };
        notebook.insert_cell_below();
        self.status = format!(
            "Inserted cell — active {} of {}.",
            notebook.active + 1,
            notebook.cells.len()
        );
    }

    /// Re-detect the analytics project (if any) and run every
    /// non-ephemeral [`BuildStep`] through DuckDB (M5.8 wire-up).
    fn run_build_all_models(&mut self) {
        let root = self.detection.root_path.clone();
        let Some(project) = self.refresh_analytics_project() else {
            self.status =
                "No `models/` directory — add one or a cockpit-analytics.toml.".to_string();
            return;
        };
        let plan: BuildPlan = match build_plan(&project) {
            Ok(plan) => plan,
            Err(err) => {
                self.status = format!("Build plan failed: {err}");
                return;
            }
        };
        let engine = DuckDbEngine::with_runner(root, self.process_arc());
        let mut ok = 0usize;
        let mut failed: Option<String> = None;
        for step in &plan.steps {
            if step.materialisation == Materialisation::Ephemeral || step.statement.is_empty() {
                continue;
            }
            match engine.execute(&step.statement) {
                Ok(_) => ok += 1,
                Err(err) => {
                    failed = Some(format!("{}: {err}", step.model));
                    break;
                }
            }
        }
        self.status = match failed {
            Some(why) => format!("Models build failed at {why} ({ok} ok before)"),
            None => format!("Models build ok — {ok} statement(s) executed."),
        };
    }

    /// Shell out to `mise exec -- quarto render <file>` for the active
    /// Quarto document (v0.5 M5.Q3). Reports the exit status in the
    /// status line; output paths the user can open are surfaced
    /// through `quarto`'s own stdout.
    fn render_quarto_document(&mut self) {
        let Some(doc) = self.document.as_ref() else {
            self.status = "No document to render.".to_string();
            return;
        };
        let is_qmd = doc
            .path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("qmd"))
            .unwrap_or(false);
        if !is_qmd {
            self.status = "Quarto: Render only works for .qmd files.".to_string();
            return;
        }
        let spec = quarto_render_spec(&doc.path, &self.detection.root_path);
        match self.process.run(&spec) {
            Ok(output) if output.success => {
                self.status = "Quarto: render complete.".to_string();
            }
            Ok(output) => {
                let snippet = output
                    .stderr_string()
                    .lines()
                    .next()
                    .unwrap_or("(no stderr)")
                    .to_string();
                self.status = format!("Quarto: render failed: {snippet}");
            }
            Err(err) => {
                self.status = format!("Quarto: could not spawn `quarto`: {err}");
            }
        }
    }

    /// Surface the DAG topological order + cycle info in the status line.
    fn show_dag_summary(&mut self) {
        let Some(project) = self.refresh_analytics_project() else {
            self.status = "No analytics project detected.".to_string();
            return;
        };
        let dag = cockpit_analytics::ModelDag::from_models(&project.models);
        match dag.topological_order() {
            Ok(order) => {
                self.status = if order.is_empty() {
                    "Analytics project: no models.".to_string()
                } else {
                    format!(
                        "Models: {} → {} ({} model(s))",
                        order.first().unwrap(),
                        order.last().unwrap(),
                        order.len()
                    )
                };
            }
            Err(err) => self.status = format!("Models DAG: {err}"),
        }
    }

    /// Re-run analytics detection. Uses `cockpit_project::StdFileSystem`
    /// plus the analytics crate's `scan_models_dir` helper to enumerate
    /// `.sql` files (the trait FS doesn't iterate directories yet —
    /// `read_dir` lives in the `scan_models_dir` adapter so detection
    /// itself stays pure). Caches the result in `self.analytics`.
    fn refresh_analytics_project(&mut self) -> Option<AnalyticsProject> {
        let root = self.detection.root_path.clone();
        let models_dir = root.join("models");
        if !models_dir.is_dir() {
            self.analytics = None;
            return None;
        }
        // Seed the fake fs with the real models so the pure
        // `detect_analytics_project` can stay path-driven.
        let model_paths = scan_models_dir(&models_dir).unwrap_or_default();
        let fs = cockpit_project::FakeFileSystem::new();
        fs.insert_dir(&root);
        fs.insert_dir(&models_dir);
        for path in &model_paths {
            if let Ok(text) = std::fs::read_to_string(path) {
                fs.insert_file(path.clone(), &text);
            }
        }
        match detect_analytics_project(&root, &fs) {
            Ok(Some(project)) => {
                self.analytics = Some(project.clone());
                Some(project)
            }
            Ok(None) => {
                self.analytics = None;
                None
            }
            Err(err) => {
                self.status = format!("Analytics detect failed: {err}");
                None
            }
        }
    }

    /// Fresh `Arc<StdProcessRunner>` for the cockpit-sql engines. The
    /// model's own `process: Box<dyn ProcessRunner>` is not cloneable;
    /// the SQL engines just spawn `mise exec` so a separate
    /// std-backed handle is fine.
    fn process_arc(&self) -> std::sync::Arc<dyn ProcessRunner> {
        std::sync::Arc::new(StdProcessRunner)
    }

    /// Drive the modal yes/no confirmation prompt from one key chord.
    fn handle_confirm_key(&mut self, chord: &KeyChord) {
        if is_chord(chord, "Escape") || is_chord(chord, "n") || is_chord(chord, "N") {
            self.confirm = None;
            self.confirm_intent = None;
            self.status = "Cancelled.".to_string();
            return;
        }
        if is_chord(chord, "y") || is_chord(chord, "Y") {
            self.accept_confirm();
            return;
        }
        if is_chord(chord, "Enter") {
            // Enter accepts only when "Yes" is highlighted — defaults to No
            // so a careless Enter is always safe (AGENTS.md rule #6).
            if self.confirm.as_ref().is_some_and(ConfirmPrompt::selection) {
                self.accept_confirm();
            } else {
                self.confirm = None;
                self.confirm_intent = None;
                self.status = "Cancelled.".to_string();
            }
            return;
        }
        let Some(prompt) = self.confirm.as_mut() else {
            return;
        };
        if is_chord(chord, "Tab")
            || is_chord(chord, "ArrowLeft")
            || is_chord(chord, "ArrowRight")
            || is_chord(chord, "h")
            || is_chord(chord, "l")
        {
            prompt.toggle();
        }
    }

    /// Insert the highlighted completion text at the current cursor.
    fn accept_completion(&mut self) {
        let Some(item) = self
            .completion
            .as_ref()
            .and_then(CompletionPopup::highlighted)
            .cloned()
        else {
            self.completion = None;
            self.status = "No completion selected.".to_string();
            return;
        };
        self.completion = None;
        let Some(doc) = self.document.as_mut() else {
            self.status = "No document open.".to_string();
            return;
        };
        let at = doc.editor.cursor().byte();
        doc.editor.replace_range(at..at, item.insert_text());
        self.status = format!("Inserted completion `{}`.", item.label);
    }

    /// Apply an LSP `WorkspaceEdit`. Edits for the open document stay in the
    /// editor buffer; edits for other files are written directly to disk.
    fn apply_workspace_edit(&mut self, edit: &serde_json::Value) -> Result<usize, String> {
        let mut by_path: HashMap<PathBuf, Vec<LspTextEdit>> = HashMap::new();
        if let Some(changes) = edit.get("changes").and_then(serde_json::Value::as_object) {
            for (uri, edits) in changes {
                let Some(path) = path_from_file_uri(uri) else {
                    return Err(format!("unsupported edit URI `{uri}`"));
                };
                collect_lsp_text_edits(edits, &mut by_path, path)?;
            }
        }
        if let Some(document_changes) = edit
            .get("documentChanges")
            .and_then(serde_json::Value::as_array)
        {
            for change in document_changes {
                let Some(uri) = change
                    .get("textDocument")
                    .and_then(|doc| doc.get("uri"))
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                let Some(path) = path_from_file_uri(uri) else {
                    return Err(format!("unsupported edit URI `{uri}`"));
                };
                if let Some(edits) = change.get("edits") {
                    collect_lsp_text_edits(edits, &mut by_path, path)?;
                }
            }
        }

        let mut applied = 0;
        for (path, mut edits) in by_path {
            edits.sort_by(|a, b| {
                b.start_line
                    .cmp(&a.start_line)
                    .then_with(|| b.start_character.cmp(&a.start_character))
            });
            if let Some(doc) = self.document.as_mut()
                && doc.path == path
            {
                applied += apply_lsp_text_edits_to_editor(&mut doc.editor, &edits)?;
                continue;
            }
            // Off-buffer edits go through the env seam (M4.10).
            let text = self
                .fs
                .read_to_string(&path)
                .map_err(|err| format!("read {}: {err}", path.display()))?;
            let mut editor = Editor::new(&text);
            applied += apply_lsp_text_edits_to_editor(&mut editor, &edits)?;
            self.fs
                .write(&path, editor.text().as_bytes())
                .map_err(|err| format!("write {}: {err}", path.display()))?;
        }
        Ok(applied)
    }

    /// Paint the whole window for one frame.
    pub fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
        self.drain_lsp_messages();
        let scale = viewport.scale.max(0.5);
        let width = viewport.width as f32 / scale;
        let height = viewport.height as f32 / scale;

        let body_height = (height - TOP_BAR_H).max(0.0);
        let computed: ComputedLayout = self.layout.compute(width as u32, body_height as u32);
        // Remember the latest layout + viewport so mouse hit tests can route
        // events to the right pane (M4.7).
        self.last_layout = Some(computed.clone());
        self.last_view_width = width;
        self.last_view_height = height;

        // Keep the PTY grid matched to the terminal pane before drawing it.
        if let Some(rect) = computed.terminal {
            self.sync_terminal_size(rect);
        }

        let mut canvas = Canvas { painter, scale };
        self.paint_top_bar(&mut canvas, width);

        if let Some(rect) = computed.files {
            self.paint_files(&mut canvas, rect, computed.focused == PaneId::Files);
        }
        self.paint_editor(
            &mut canvas,
            computed.editor,
            computed.focused == PaneId::Editor,
        );
        if let Some(rect) = computed.terminal {
            self.paint_terminal(&mut canvas, rect, computed.focused == PaneId::Terminal);
        }

        if let Some(palette) = &self.palette {
            paint_palette(&mut canvas, &self.theme, palette, width, height);
        }
        if let Some(finder) = &self.finder {
            paint_finder(&mut canvas, &self.theme, finder, width, height);
        }
        if let Some(completion) = &self.completion {
            paint_completion(&mut canvas, &self.theme, completion, width, height);
        }
        if let Some(prompt) = &self.confirm {
            paint_confirm(&mut canvas, &self.theme, prompt, width, height);
        }
    }

    fn paint_top_bar(&self, canvas: &mut Canvas<'_>, width: f32) {
        canvas.rect(0.0, 0.0, width, TOP_BAR_H, self.theme.pane_background);
        canvas.rect(0.0, TOP_BAR_H - 1.0, width, 1.0, self.theme.pane_border);
        let title = format!("Coding Cockpit  ·  {}", self.detection.display_name);
        canvas.text(PAD, 8.0, title, self.theme.text, FONT);
        canvas.text(
            width * 0.42,
            8.0,
            self.status.clone(),
            self.theme.muted_text,
            FONT,
        );
    }

    /// Paint a pane frame and return its inner content rectangle (logical).
    fn paint_pane(
        &self,
        canvas: &mut Canvas<'_>,
        rect: UiRect,
        title: &str,
        focused: bool,
    ) -> ContentRect {
        let x = rect.x as f32;
        let y = rect.y as f32 + TOP_BAR_H;
        let w = rect.width as f32;
        let h = rect.height as f32;

        canvas.rect(x, y, w, h, self.theme.pane_background);
        if focused {
            canvas.rect(x, y, w, 2.0, self.theme.accent);
        }
        canvas.rect(x, y + HEADER_H - 1.0, w, 1.0, self.theme.pane_border);
        canvas.rect(x + w - 1.0, y, 1.0, h, self.theme.pane_border);

        let label_color = if focused {
            self.theme.text
        } else {
            self.theme.muted_text
        };
        canvas.text(x + PAD, y + 6.0, title, label_color, FONT - 1.0);

        ContentRect {
            x,
            y: y + HEADER_H,
            w,
            h: h - HEADER_H,
        }
    }

    fn paint_files(&self, canvas: &mut Canvas<'_>, rect: UiRect, focused: bool) {
        let content = self.paint_pane(canvas, rect, "FILES", focused);
        let top = content.y + PAD * 0.5;
        let visible = ((content.h - PAD) / ROW_H).max(0.0) as usize;

        for (index, row) in self.browser.rows().iter().take(visible).enumerate() {
            let row_y = top + index as f32 * ROW_H;
            if index == self.browser.selected_index() {
                canvas.rect(
                    content.x + 2.0,
                    row_y,
                    content.w - 4.0,
                    ROW_H,
                    self.theme.selection,
                );
                if focused {
                    canvas.rect(content.x, row_y, 2.0, ROW_H, self.theme.accent);
                }
            }
            let marker = match (row.kind, row.expanded) {
                (FileNodeKind::Directory, true) => "v ",
                (FileNodeKind::Directory, false) => "> ",
                (FileNodeKind::File, _) => "  ",
            };
            let badge = row.git_status.map(|status| status.badge()).unwrap_or(' ');
            let text_x = content.x + PAD + row.depth as f32 * INDENT_W;
            let color = if row.is_dir() {
                self.theme.text
            } else {
                self.theme.muted_text
            };
            canvas.text(
                text_x,
                row_y + 3.0,
                format!("{badge} {marker}{}", row.name),
                color,
                FONT,
            );
        }
    }

    fn paint_editor(&self, canvas: &mut Canvas<'_>, rect: UiRect, focused: bool) {
        let title = match &self.document {
            Some(doc) => format!("EDITOR  ·  {}", doc.name),
            None => "EDITOR".to_string(),
        };
        let content = self.paint_pane(canvas, rect, &title, focused);

        match &self.document {
            Some(doc) => self.paint_document(canvas, &content, doc),
            None => self.paint_welcome(canvas, &content),
        }
    }

    fn paint_document(&self, canvas: &mut Canvas<'_>, content: &ContentRect, doc: &OpenDocument) {
        let editor = &doc.editor;
        let buffer = editor.buffer();
        let mode_line_h = ROW_H;
        let text_height = (content.h - mode_line_h).max(0.0);
        let visible = ((text_height - PAD) / ROW_H).max(1.0) as usize;

        let (cursor_line, cursor_col) = editor.cursor().line_col(buffer);
        // The first visible line is whichever brings the cursor into view —
        // typed cursor moves still auto-scroll. The mouse wheel can push the
        // viewport above that line (M4.7); the user-driven offset wins as
        // long as the cursor is still inside the visible range.
        let cursor_anchor = cursor_line.saturating_sub(visible.saturating_sub(1));
        let scroll_anchor = self.editor_scroll.round() as usize;
        let max_first = buffer.len_lines().saturating_sub(1);
        let first = if scroll_anchor <= cursor_line
            && cursor_line < scroll_anchor + visible
            && scroll_anchor <= max_first
        {
            scroll_anchor
        } else {
            cursor_anchor
        };

        let text = buffer.text();
        let lines: Vec<&str> = text.split('\n').collect();
        let top = content.y + PAD * 0.5;
        let char_w = FONT * CHAR_W_RATIO;

        // Diagnostics keyed by starting line for cheap per-row lookup.
        let mut diagnostics_by_line: HashMap<u32, Vec<&Diagnostic>> = HashMap::new();
        for diagnostic in self.diagnostics_for(&doc.path) {
            diagnostics_by_line
                .entry(diagnostic.range.start.line)
                .or_default()
                .push(diagnostic);
        }

        if let Some((sel_start, sel_end)) = editor.selection() {
            for row in 0..visible {
                let line_index = first + row;
                let Some(line) = lines.get(line_index) else {
                    break;
                };
                let line_start = buffer.line_to_byte(line_index);
                let line_end = line_start + line.len();
                if sel_end <= line_start || sel_start > line_end {
                    continue;
                }
                let from = sel_start.saturating_sub(line_start).min(line.len());
                let to = (sel_end - line_start).min(line.len());
                let start_col = line[..from].chars().count();
                let end_col = line[..to].chars().count();
                // A linewise selection running past this line still shows a sliver.
                let extends_past = sel_end > line_end;
                let cols = (end_col - start_col).max(usize::from(extends_past));
                if cols == 0 {
                    continue;
                }
                canvas.rect(
                    content.x + GUTTER_W + start_col as f32 * char_w,
                    top + row as f32 * ROW_H,
                    cols as f32 * char_w,
                    ROW_H,
                    self.theme.selection,
                );
            }
        }

        for row in 0..visible {
            let line_index = first + row;
            let Some(line) = lines.get(line_index) else {
                break;
            };
            let line_y = top + row as f32 * ROW_H + 3.0;
            canvas.text(
                content.x + PAD,
                line_y,
                format!("{:>4}", line_index + 1),
                self.theme.muted_text,
                FONT,
            );

            // Diagnostic marker + inline message for this line, if any.
            if let Some(diagnostics) = diagnostics_by_line.get(&(line_index as u32))
                && let Some(strongest) = diagnostics
                    .iter()
                    .min_by_key(|d| d.severity.unwrap_or(cockpit_lsp::DiagnosticSeverity::ERROR))
            {
                let color = self.diagnostic_color(strongest.severity);
                canvas.rect(
                    content.x + GUTTER_W - 6.0,
                    line_y - 3.0,
                    3.0,
                    ROW_H - 2.0,
                    color,
                );
                let line_pixel_width = line.chars().count() as f32 * char_w;
                let mut message = strongest.message.lines().next().unwrap_or("").to_string();
                let max_chars = 80;
                if message.chars().count() > max_chars {
                    message = message.chars().take(max_chars).collect::<String>() + "…";
                }
                canvas.text(
                    content.x + GUTTER_W + line_pixel_width + PAD * 2.0,
                    line_y,
                    message,
                    color,
                    FONT - 1.0,
                );
            }

            self.paint_code_line(
                canvas,
                content.x + GUTTER_W,
                line_y,
                line,
                buffer.line_to_byte(line_index),
                editor.highlights(),
            );
        }

        if cursor_line >= first && cursor_line < first + visible {
            let row = cursor_line - first;
            let line = lines.get(cursor_line).copied().unwrap_or("");
            let split = cursor_col.min(line.len());
            let char_col = line[..split].chars().count();
            let cursor_x = content.x + GUTTER_W + char_col as f32 * char_w;
            let cursor_y = top + row as f32 * ROW_H;
            let insert = editor.mode() == Mode::Insert;
            if insert {
                canvas.rect(cursor_x, cursor_y, 2.0, ROW_H, self.theme.accent);
            } else {
                canvas.rect(
                    cursor_x,
                    cursor_y,
                    char_w.max(6.0),
                    ROW_H,
                    self.theme.cursor,
                );
                if let Some(under) = line[split..].chars().next() {
                    canvas.text(
                        cursor_x,
                        cursor_y + 3.0,
                        under.to_string(),
                        self.theme.background,
                        FONT,
                    );
                }
            }
        }

        self.paint_mode_line(canvas, content, editor, cursor_line, cursor_col);
    }

    /// Draw one buffer line, splitting it into themed runs per highlight span.
    /// `line_start` is the line's byte offset in the full buffer.
    fn paint_code_line(
        &self,
        canvas: &mut Canvas<'_>,
        base_x: f32,
        y: f32,
        line: &str,
        line_start: usize,
        highlights: &[HighlightSpan],
    ) {
        let char_w = FONT * CHAR_W_RATIO;
        let mut byte = 0usize;
        let mut col = 0usize;
        while byte < line.len() {
            let abs = line_start + byte;
            let (seg_end, color) = match highlights
                .iter()
                .find(|span| span.range.start <= abs && abs < span.range.end)
            {
                Some(span) => (
                    (span.range.end - line_start).min(line.len()),
                    self.syntax_color(span.kind),
                ),
                None => {
                    // Run plain text up to wherever the next highlight begins.
                    let next = highlights
                        .iter()
                        .map(|span| span.range.start)
                        .filter(|&start| start > abs)
                        .min()
                        .map(|start| (start - line_start).min(line.len()))
                        .unwrap_or(line.len());
                    (next, self.theme.text)
                }
            };
            let segment = &line[byte..seg_end];
            canvas.text(
                base_x + col as f32 * char_w,
                y,
                segment.to_string(),
                color,
                FONT,
            );
            col += segment.chars().count();
            byte = seg_end;
        }
    }

    fn syntax_color(&self, kind: HighlightKind) -> Color {
        let syntax = &self.theme.syntax;
        match kind {
            HighlightKind::Keyword => syntax.keyword,
            HighlightKind::Function => syntax.function,
            HighlightKind::Type => syntax.type_name,
            HighlightKind::String => syntax.string,
            HighlightKind::Comment => syntax.comment,
            HighlightKind::Constant => syntax.constant,
            HighlightKind::Variable => syntax.variable,
            HighlightKind::Operator => syntax.operator,
            HighlightKind::Attribute => syntax.attribute,
            HighlightKind::Punctuation => syntax.punctuation,
        }
    }

    fn diagnostic_color(&self, severity: Option<DiagnosticSeverity>) -> Color {
        match severity {
            Some(s) if s == DiagnosticSeverity::WARNING => self.theme.diagnostic_warning,
            Some(s) if s == DiagnosticSeverity::INFORMATION => self.theme.diagnostic_info,
            Some(s) if s == DiagnosticSeverity::HINT => self.theme.diagnostic_hint,
            _ => self.theme.diagnostic_error,
        }
    }

    fn paint_mode_line(
        &self,
        canvas: &mut Canvas<'_>,
        content: &ContentRect,
        editor: &Editor,
        cursor_line: usize,
        cursor_col: usize,
    ) {
        let y = content.y + content.h - ROW_H;
        canvas.rect(content.x, y, content.w, ROW_H, self.theme.pane_border);

        let mode = match editor.mode() {
            Mode::Normal => "NORMAL".to_string(),
            Mode::Insert => "INSERT".to_string(),
            Mode::Visual => "VISUAL".to_string(),
            Mode::VisualLine => "VISUAL LINE".to_string(),
            Mode::Replace => "REPLACE".to_string(),
            Mode::Command => format!("COMMAND  :{}", editor.vim().command_line()),
            Mode::Search => format!("SEARCH  /{}", editor.vim().search_query()),
        };
        let dirty = if editor.is_dirty() { "  [+]" } else { "" };
        canvas.text(
            content.x + PAD,
            y + 3.0,
            format!("{mode}{dirty}"),
            self.theme.text,
            FONT - 1.0,
        );
        canvas.text(
            content.x + content.w - 84.0,
            y + 3.0,
            format!("{}:{}", cursor_line + 1, cursor_col + 1),
            self.theme.muted_text,
            FONT - 1.0,
        );
    }

    fn paint_welcome(&self, canvas: &mut Canvas<'_>, content: &ContentRect) {
        let mut lines: Vec<(String, Color)> = vec![
            ("Coding Cockpit — v0.1 shell".to_string(), self.theme.text),
            (String::new(), self.theme.text),
            (
                format!("Project    {}", self.detection.display_name),
                self.theme.text,
            ),
            (
                format!("Root       {}", self.detection.root_path.display()),
                self.theme.muted_text,
            ),
            (
                format!(
                    "Detected   {}",
                    match self.detection.strongest_signal {
                        Some(kind) => format!("{kind:?}"),
                        None => "no project signals".to_string(),
                    }
                ),
                self.theme.muted_text,
            ),
        ];

        let tasks = &self.detection.mise.tasks;
        if tasks.is_empty() {
            lines.push((
                "Mise       no tasks detected".to_string(),
                self.theme.muted_text,
            ));
        } else {
            lines.push((
                format!("Mise       {} task(s):", tasks.len()),
                self.theme.muted_text,
            ));
            for task in tasks.iter().take(6) {
                lines.push((
                    format!("             · {}", task.name),
                    self.theme.muted_text,
                ));
            }
        }

        lines.push((String::new(), self.theme.text));
        lines.push((
            "Select a file on the left and press Enter to edit it with Vim keys.".to_string(),
            self.theme.muted_text,
        ));
        lines.push((
            "Ctrl+h/j/l  focus panes      Ctrl+`  toggle terminal".to_string(),
            self.theme.muted_text,
        ));
        lines.push((
            "Ctrl+b  toggle files         Ctrl+Shift+P  palette      Ctrl+q  quit".to_string(),
            self.theme.muted_text,
        ));

        let top = content.y + PAD;
        for (index, (line, color)) in lines.iter().enumerate() {
            canvas.text(
                content.x + PAD,
                top + index as f32 * ROW_H + 3.0,
                line.clone(),
                *color,
                FONT,
            );
        }
    }

    fn paint_terminal(&self, canvas: &mut Canvas<'_>, rect: UiRect, focused: bool) {
        let content = self.paint_pane(canvas, rect, "TERMINAL", focused);
        let pane_rects = self.mux_session.active_window().pane_rects(
            MuxRect::new(
                content.x.max(0.0) as u32,
                content.y.max(0.0) as u32,
                content.w.max(0.0) as u32,
                content.h.max(0.0) as u32,
            ),
            1,
        );
        if pane_rects.len() == 1 && !self.terminals.contains_key(&pane_rects[0].pane) {
            self.paint_terminal_placeholder(canvas, &content);
            return;
        }

        for pane in pane_rects {
            if self.mux_session.active_window().layout.leaves().len() > 1 {
                self.paint_mux_pane_frame(canvas, pane, focused);
            }
            match self.terminals.get(&pane.pane) {
                Some(terminal) => {
                    self.paint_terminal_grid(canvas, pane, terminal, focused && pane.active)
                }
                None => self.paint_mux_placeholder(canvas, pane, "pending PTY"),
            }
        }
    }

    fn paint_terminal_placeholder(&self, canvas: &mut Canvas<'_>, content: &ContentRect) {
        let lines = [
            "Integrated terminal",
            "",
            "Focus this pane (Ctrl+L) to start a session.",
            "Runs the project's Zellij workspace when mise and",
            "zellij are on PATH; otherwise a plain shell.",
        ];
        let top = content.y + PAD;
        for (index, line) in lines.iter().enumerate() {
            canvas.text(
                content.x + PAD,
                top + index as f32 * ROW_H + 3.0,
                *line,
                self.theme.muted_text,
                FONT,
            );
        }
    }

    fn paint_mux_pane_frame(
        &self,
        canvas: &mut Canvas<'_>,
        pane: cockpit_mux::PaneRect,
        terminal_focused: bool,
    ) {
        let rect = pane.rect;
        let x = rect.x as f32;
        let y = rect.y as f32;
        let w = rect.width as f32;
        let h = rect.height as f32;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let border = if pane.active && terminal_focused {
            self.theme.accent
        } else {
            self.theme.pane_border
        };
        canvas.rect(x, y, w, 1.0, border);
        canvas.rect(x, y + h - 1.0, w, 1.0, border);
        canvas.rect(x, y, 1.0, h, border);
        canvas.rect(x + w - 1.0, y, 1.0, h, border);
        canvas.text(
            x + 4.0,
            y + 3.0,
            pane.pane.to_string(),
            if pane.active {
                self.theme.text
            } else {
                self.theme.muted_text
            },
            FONT - 2.0,
        );
    }

    fn paint_mux_placeholder(
        &self,
        canvas: &mut Canvas<'_>,
        pane: cockpit_mux::PaneRect,
        label: &str,
    ) {
        canvas.text(
            pane.rect.x as f32 + PAD,
            pane.rect.y as f32 + ROW_H + 4.0,
            label,
            self.theme.muted_text,
            FONT,
        );
    }

    fn paint_terminal_grid(
        &self,
        canvas: &mut Canvas<'_>,
        pane: cockpit_mux::PaneRect,
        terminal: &LiveTerminal,
        focused: bool,
    ) {
        let content_x = pane.rect.x as f32;
        let content_y = pane.rect.y as f32;
        let snapshot = terminal.snapshot();
        let grid = &snapshot.grid;
        let char_w = FONT * CHAR_W_RATIO;
        let top = content_y + PAD * 0.5;

        for row in 0..grid.height() {
            let Some(text) = grid.row_text(row) else {
                continue;
            };
            let trimmed = text.trim_end();
            if !trimmed.is_empty() {
                canvas.text(
                    content_x + PAD,
                    top + row as f32 * ROW_H + 3.0,
                    trimmed.to_string(),
                    self.theme.text,
                    FONT,
                );
            }
        }

        let cursor = grid.cursor();
        let cursor_x = content_x + PAD + cursor.col as f32 * char_w;
        let cursor_y = top + cursor.row as f32 * ROW_H;
        let cursor_color = if focused {
            self.theme.cursor
        } else {
            self.theme.muted_text
        };
        canvas.rect(cursor_x, cursor_y, char_w.max(6.0), ROW_H, cursor_color);
        if let Some(cell) = grid.cell(cursor.row, cursor.col)
            && cell.ch != ' '
        {
            canvas.text(
                cursor_x,
                cursor_y + 3.0,
                cell.ch.to_string(),
                self.theme.background,
                FONT,
            );
        }

        let notice = match &snapshot.status {
            TerminalStatus::Running => None,
            TerminalStatus::Exited => Some("[process exited]".to_string()),
            TerminalStatus::Failed(error) => Some(format!("[terminal error: {error}]")),
        };
        if let Some(notice) = notice {
            canvas.text(
                content_x + PAD,
                content_y + pane.rect.height as f32 - ROW_H + 3.0,
                notice,
                self.theme.muted_text,
                FONT,
            );
        }
    }
}

/// Inner (content) rectangle of a pane, in logical pixels.
struct ContentRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// Painter wrapper that scales logical coordinates to physical pixels.
struct Canvas<'p> {
    painter: &'p mut Painter,
    scale: f32,
}

impl Canvas<'_> {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.painter.rect(
            RenderRect::new(
                x * self.scale,
                y * self.scale,
                w * self.scale,
                h * self.scale,
            ),
            color,
        );
    }

    fn text(&mut self, x: f32, y: f32, text: impl Into<String>, color: Color, size: f32) {
        self.painter.text(
            x * self.scale,
            y * self.scale,
            text,
            color,
            size * self.scale,
        );
    }
}

/// True when `position` falls inside `rect`. Pane rectangles use logical
/// pixels in body coordinates (i.e. the top bar is at `y < TOP_BAR_H` and
/// not part of any pane); callers compensate before calling.
fn pane_contains(rect: UiRect, position: PointerPosition) -> bool {
    let x0 = rect.x as f32;
    let y0 = rect.y as f32 + TOP_BAR_H;
    let x1 = x0 + rect.width as f32;
    let y1 = y0 + rect.height as f32;
    position.x >= x0 && position.x < x1 && position.y >= y0 && position.y < y1
}

fn mux_rect_contains(rect: MuxRect, position: PointerPosition) -> bool {
    let x0 = rect.x as f32;
    let y0 = rect.y as f32;
    let x1 = x0 + rect.width as f32;
    let y1 = y0 + rect.height as f32;
    position.x >= x0 && position.x < x1 && position.y >= y0 && position.y < y1
}

/// True when `chord` is a single unmodified press of `key`.
fn is_chord(chord: &KeyChord, key: &str) -> bool {
    *chord == KeyChord::single(key, Modifiers::NONE)
}

/// Translate a single-stroke key chord into a Vim state-machine key.
///
/// Returns `None` for chords the Vim FSM has no representation for (multi-key
/// chords, Alt/Meta combos, named keys like Tab or the arrows).
fn chord_to_vim_key(chord: &KeyChord) -> Option<VimKey> {
    let [stroke] = chord.strokes() else {
        return None;
    };
    let modifiers = stroke.modifiers();
    if modifiers.alt || modifiers.meta {
        return None;
    }
    match stroke.key() {
        "Escape" => Some(VimKey::Escape),
        "Enter" => Some(VimKey::Enter),
        "Backspace" => Some(VimKey::Backspace),
        "Space" if !modifiers.ctrl => Some(VimKey::Char(' ')),
        key => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if modifiers.ctrl {
                Some(VimKey::Ctrl(c))
            } else if c.is_alphabetic() && modifiers.shift {
                Some(VimKey::Char(c.to_ascii_uppercase()))
            } else {
                Some(VimKey::Char(c))
            }
        }
    }
}

/// Extract a plain typed character from a single-stroke chord.
///
/// Returns `None` for command-modifier combos and named keys, so the palette
/// query only ever grows by literal characters.
fn chord_to_char(chord: &KeyChord) -> Option<char> {
    let [stroke] = chord.strokes() else {
        return None;
    };
    let modifiers = stroke.modifiers();
    if modifiers.ctrl || modifiers.alt || modifiers.meta {
        return None;
    }
    if stroke.key() == "Space" {
        return Some(' ');
    }
    let mut chars = stroke.key().chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(if c.is_alphabetic() && modifiers.shift {
        c.to_ascii_uppercase()
    } else {
        c
    })
}

/// Encode a single-stroke chord as the bytes to write to the PTY.
///
/// Returns `None` for chords with no terminal representation (Alt/Meta combos,
/// non-letter Ctrl combos, unmapped named keys).
fn chord_to_terminal_bytes(chord: &KeyChord) -> Option<Vec<u8>> {
    let [stroke] = chord.strokes() else {
        return None;
    };
    let modifiers = stroke.modifiers();
    if modifiers.alt || modifiers.meta {
        return None;
    }
    match stroke.key() {
        "Enter" => Some(vec![b'\r']),
        "Backspace" => Some(vec![0x7f]),
        "Tab" => Some(vec![b'\t']),
        "Escape" => Some(vec![0x1b]),
        "Space" => Some(vec![b' ']),
        "ArrowUp" => Some(b"\x1b[A".to_vec()),
        "ArrowDown" => Some(b"\x1b[B".to_vec()),
        "ArrowRight" => Some(b"\x1b[C".to_vec()),
        "ArrowLeft" => Some(b"\x1b[D".to_vec()),
        key => {
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            if modifiers.ctrl {
                // Ctrl+letter → the C0 control byte (Ctrl+C = 0x03, …).
                c.is_ascii_alphabetic()
                    .then(|| vec![(c.to_ascii_uppercase() as u8) & 0x1f])
            } else {
                let c = if c.is_alphabetic() && modifiers.shift {
                    c.to_ascii_uppercase()
                } else {
                    c
                };
                Some(c.to_string().into_bytes())
            }
        }
    }
}

/// Build the spawn argv for `config`'s language server, wrapping it in
/// `mise exec --` so every server inherits the project's mise environment
/// (spec §19 / M4.5). Result: `["mise", "exec", "--", <command>, <args>...]`.
/// Pick a notebook view-model when `path`'s contents look like a
/// Jupytext or Quarto source. The two file-level cues are:
///
/// * `.qmd` extension → Quarto, parsed with [`parse_quarto`].
/// * `.sql` / `.ggsql` extension with at least one `-- %% cell` marker
///   → Jupytext, parsed with [`parse_notebook`].
///
/// Returns `None` for plain text files so opening a regular `.sql`
/// file without markers keeps the normal editor experience.
fn recognise_notebook(path: &Path, content: &str) -> Option<Notebook> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    match ext.as_deref() {
        Some("qmd") => parse_quarto(content).ok(),
        Some("sql") | Some("ggsql") if is_notebook_source(content) => {
            let default_kind = if ext.as_deref() == Some("ggsql") {
                CellKind::Ggsql
            } else {
                CellKind::Sql
            };
            parse_notebook(content, default_kind).ok()
        }
        _ => None,
    }
}

/// Route one cell's source through the right SQL engine based on its
/// [`CellKind`]. Both engines share the M5.1 `SqlEngine` trait;
/// production callers always spawn through `StdProcessRunner` because
/// these are external `mise exec` invocations.
fn run_cell_against_engines(
    root: &Path,
    kind: CellKind,
    source: &str,
) -> Result<cockpit_sql::QueryResult, cockpit_sql::QueryError> {
    let process = std::sync::Arc::new(StdProcessRunner) as std::sync::Arc<dyn ProcessRunner>;
    match kind {
        CellKind::Ggsql => GgsqlEngine::with_runner(root.to_path_buf(), process).execute(source),
        CellKind::Sql if statement_targets_ggsql(source) => {
            GgsqlEngine::with_runner(root.to_path_buf(), process).execute(source)
        }
        CellKind::Sql => DuckDbEngine::with_runner(root.to_path_buf(), process).execute(source),
        CellKind::Markdown => Ok(cockpit_sql::QueryResult::empty()),
    }
}

fn lsp_launch_argv(config: &ServerConfig) -> Vec<String> {
    let inner: Vec<&str> = std::iter::once(config.command.as_str())
        .chain(config.args.iter().map(String::as_str))
        .collect();
    mise_exec_command(&inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LspTextEdit {
    start_line: usize,
    start_character: usize,
    end_line: usize,
    end_character: usize,
    new_text: String,
}

fn collect_lsp_text_edits(
    value: &serde_json::Value,
    by_path: &mut HashMap<PathBuf, Vec<LspTextEdit>>,
    path: PathBuf,
) -> Result<(), String> {
    let Some(items) = value.as_array() else {
        return Err("workspace edit entry is not an array".to_string());
    };
    for item in items {
        by_path
            .entry(path.clone())
            .or_default()
            .push(parse_lsp_text_edit(item)?);
    }
    Ok(())
}

fn parse_lsp_text_edit(value: &serde_json::Value) -> Result<LspTextEdit, String> {
    let range = value
        .get("range")
        .ok_or_else(|| "text edit missing range".to_string())?;
    let start = range
        .get("start")
        .ok_or_else(|| "text edit missing start".to_string())?;
    let end = range
        .get("end")
        .ok_or_else(|| "text edit missing end".to_string())?;
    let new_text = value
        .get("newText")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "text edit missing newText".to_string())?;
    Ok(LspTextEdit {
        start_line: lsp_position_field(start, "line")?,
        start_character: lsp_position_field(start, "character")?,
        end_line: lsp_position_field(end, "line")?,
        end_character: lsp_position_field(end, "character")?,
        new_text: new_text.to_string(),
    })
}

fn lsp_position_field(value: &serde_json::Value, field: &str) -> Result<usize, String> {
    let raw = value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("LSP position missing `{field}`"))?;
    usize::try_from(raw).map_err(|_| format!("LSP position `{field}` is too large"))
}

fn apply_lsp_text_edits_to_editor(
    editor: &mut Editor,
    edits: &[LspTextEdit],
) -> Result<usize, String> {
    for edit in edits {
        let start = editor
            .buffer()
            .line_col_to_byte(edit.start_line, edit.start_character);
        let end = editor
            .buffer()
            .line_col_to_byte(edit.end_line, edit.end_character);
        if start > end {
            return Err("text edit range starts after it ends".to_string());
        }
        editor.replace_range(start..end, &edit.new_text);
    }
    Ok(edits.len())
}

fn symbol_under_cursor(text: &str, cursor: usize) -> Option<String> {
    if text.is_empty() {
        return None;
    }
    let cursor = cursor.min(text.len());
    let mut start = cursor;
    while start > 0 {
        let prev = text[..start].chars().next_back()?;
        if !is_symbol_char(prev) {
            break;
        }
        start -= prev.len_utf8();
    }
    let mut end = cursor;
    while end < text.len() {
        let next = text[end..].chars().next()?;
        if !is_symbol_char(next) {
            break;
        }
        end += next.len_utf8();
    }
    (start < end).then(|| text[start..end].to_string())
}

fn is_symbol_char(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

fn is_rename_input_char(c: char) -> bool {
    c == '_' || c == '-' || c.is_ascii_alphanumeric()
}

fn parse_completion_item(value: &serde_json::Value) -> Option<CompletionItem> {
    let label = value.get("label")?.as_str()?.to_string();
    let mut item = CompletionItem::new(label);
    item.detail = value
        .get("detail")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    item.documentation = completion_documentation(value.get("documentation"));
    item.insert_text = value
        .get("insertText")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("textEdit")
                .and_then(|edit| edit.get("newText"))
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_string);
    Some(item)
}

fn completion_documentation(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) if !text.is_empty() => Some(text.clone()),
        serde_json::Value::Object(map) => map
            .get("value")
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// Format `path` as an LSP `file://` URI. The path is treated as absolute;
/// callers above resolve project-relative paths against the project root.
fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if cfg!(windows) {
        let normalised = raw.replace('\\', "/");
        if normalised.starts_with('/') {
            format!("file://{normalised}")
        } else {
            format!("file:///{normalised}")
        }
    } else {
        format!("file://{raw}")
    }
}

/// Inverse of [`file_uri`] — pull a [`PathBuf`] out of a `file://` URI.
/// Returns `None` when the URI is not a file scheme cockpit can reverse.
fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let trimmed = uri.strip_prefix("file://")?;
    if cfg!(windows) {
        // Windows file URIs round-trip as `file:///C:/...` — strip the leading
        // slash and put backslashes back.
        let body = trimmed.strip_prefix('/').unwrap_or(trimmed);
        Some(PathBuf::from(body.replace('/', "\\")))
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Extract the first usable target from a `textDocument/definition` result,
/// which the LSP spec lets be `null`, a single `Location`, an array of
/// `Location`, or an array of `LocationLink`. Returns `(uri, line, character)`
/// in 0-based LSP coordinates.
fn parse_first_location(result: &serde_json::Value) -> Option<(String, u32, u32)> {
    if result.is_null() {
        return None;
    }
    if let Some(arr) = result.as_array() {
        return arr.iter().find_map(parse_first_location);
    }
    // Location: { uri, range }. LocationLink: { targetUri,
    // targetSelectionRange | targetRange }.
    let uri = result
        .get("uri")
        .and_then(|v| v.as_str())
        .or_else(|| result.get("targetUri").and_then(|v| v.as_str()))?;
    let range = result
        .get("range")
        .or_else(|| result.get("targetSelectionRange"))
        .or_else(|| result.get("targetRange"))?;
    let start = range.get("start")?;
    let line = start.get("line")?.as_u64()?;
    let character = start.get("character")?.as_u64()?;
    Some((uri.to_string(), line as u32, character as u32))
}

/// Extract a single plain-text string from a `textDocument/hover` result,
/// which can be `null`, or `{ contents: <MarkupContent | MarkedString |
/// MarkedString[]> }`. Markdown formatting is preserved as-is for now;
/// richer rendering is a later milestone.
fn extract_hover_contents(result: &serde_json::Value) -> Option<String> {
    if result.is_null() {
        return None;
    }
    let contents = result.get("contents")?;
    extract_marked_string(contents)
}

fn extract_marked_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::String(_) => None,
        serde_json::Value::Object(map) => {
            // MarkupContent { kind, value } or MarkedString { language, value }.
            map.get("value")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        }
        serde_json::Value::Array(arr) => {
            let joined: Vec<String> = arr.iter().filter_map(extract_marked_string).collect();
            (!joined.is_empty()).then(|| joined.join("\n\n"))
        }
        _ => None,
    }
}

fn mux_select_window_index(id: &str) -> Option<usize> {
    mux_command_ids::SELECT_WINDOW
        .iter()
        .position(|candidate| *candidate == id)
}

/// The command palette's v0.1 command set (spec §16).
fn palette_entries() -> Vec<PaletteEntry> {
    vec![
        PaletteEntry::new(command_ids::SAVE, "File: Save"),
        PaletteEntry::new(command_ids::FUZZY_OPEN, "File: Fuzzy Open"),
        PaletteEntry::new(command_ids::FOCUS_FILES, "View: Focus Files"),
        PaletteEntry::new(command_ids::FOCUS_EDITOR, "View: Focus Editor"),
        PaletteEntry::new(command_ids::FOCUS_TERMINAL, "View: Focus Terminal"),
        PaletteEntry::new(command_ids::TOGGLE_FILES, "View: Toggle Files Pane"),
        PaletteEntry::new(command_ids::TOGGLE_TERMINAL, "View: Toggle Terminal Pane"),
        PaletteEntry::new(MISE_RUN_TASK, "Mise: Run Task"),
        PaletteEntry::new(TERMINAL_OPEN_PATH, "Terminal: Open Path"),
        PaletteEntry::new(TERMINAL_SEND_FILE_PATH, "Terminal: Send Current File Path"),
        PaletteEntry::new(TERMINAL_SEND_SELECTION, "Terminal: Send Selection"),
        PaletteEntry::new(mux_command_ids::NEW_WINDOW, "Mux: New Window"),
        PaletteEntry::new(mux_command_ids::KILL_WINDOW, "Mux: Kill Window"),
        PaletteEntry::new(mux_command_ids::NEXT_WINDOW, "Mux: Next Window"),
        PaletteEntry::new(mux_command_ids::PREVIOUS_WINDOW, "Mux: Previous Window"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_0, "Mux: Select Window 0"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_1, "Mux: Select Window 1"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_2, "Mux: Select Window 2"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_3, "Mux: Select Window 3"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_4, "Mux: Select Window 4"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_5, "Mux: Select Window 5"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_6, "Mux: Select Window 6"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_7, "Mux: Select Window 7"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_8, "Mux: Select Window 8"),
        PaletteEntry::new(mux_command_ids::SELECT_WINDOW_9, "Mux: Select Window 9"),
        PaletteEntry::new(mux_command_ids::SPLIT_HORIZONTAL, "Mux: Split Horizontal"),
        PaletteEntry::new(mux_command_ids::SPLIT_VERTICAL, "Mux: Split Vertical"),
        PaletteEntry::new(mux_command_ids::KILL_PANE, "Mux: Kill Pane"),
        PaletteEntry::new(mux_command_ids::NEXT_PANE, "Mux: Next Pane"),
        PaletteEntry::new(mux_command_ids::LAST_PANE, "Mux: Last Pane"),
        PaletteEntry::new(mux_command_ids::SWAP_PANE_NEXT, "Mux: Swap Pane Next"),
        PaletteEntry::new(mux_command_ids::ZOOM_PANE, "Mux: Zoom Pane"),
        PaletteEntry::new(mux_command_ids::FOCUS_UP, "Mux: Focus Up"),
        PaletteEntry::new(mux_command_ids::FOCUS_DOWN, "Mux: Focus Down"),
        PaletteEntry::new(mux_command_ids::FOCUS_LEFT, "Mux: Focus Left"),
        PaletteEntry::new(mux_command_ids::FOCUS_RIGHT, "Mux: Focus Right"),
        PaletteEntry::new(mux_command_ids::RESIZE_UP, "Mux: Resize Up"),
        PaletteEntry::new(mux_command_ids::RESIZE_DOWN, "Mux: Resize Down"),
        PaletteEntry::new(mux_command_ids::RESIZE_LEFT, "Mux: Resize Left"),
        PaletteEntry::new(mux_command_ids::RESIZE_RIGHT, "Mux: Resize Right"),
        PaletteEntry::new(mux_command_ids::NEXT_LAYOUT, "Mux: Next Layout"),
        PaletteEntry::new(TEST_RUN_ALL, "Test: Run All"),
        PaletteEntry::new(TEST_RUN_CURRENT_FILE, "Test: Run Current File"),
        PaletteEntry::new(TEST_RUN_NEAREST, "Test: Run Nearest"),
        PaletteEntry::new(LSP_GOTO_DEFINITION, "LSP: Go to Definition"),
        PaletteEntry::new(LSP_SHOW_HOVER, "LSP: Show Hover"),
        PaletteEntry::new(LSP_RENAME, "LSP: Rename Symbol"),
        PaletteEntry::new(LSP_COMPLETION, "LSP: Complete"),
        PaletteEntry::new(LSP_CODE_ACTION, "LSP: Code Action"),
        PaletteEntry::new(EDITOR_FORMAT, "Editor: Format Document"),
        PaletteEntry::new(
            EDITOR_TOGGLE_FORMAT_ON_SAVE,
            "Editor: Toggle Format on Save",
        ),
        PaletteEntry::new(NOTEBOOK_RUN_ACTIVE_CELL, "Notebook: Run Active Cell"),
        PaletteEntry::new(NOTEBOOK_NEXT_CELL, "Notebook: Next Cell"),
        PaletteEntry::new(NOTEBOOK_PREVIOUS_CELL, "Notebook: Previous Cell"),
        PaletteEntry::new(NOTEBOOK_INSERT_CELL_BELOW, "Notebook: Insert Cell Below"),
        PaletteEntry::new(MODELS_BUILD_ALL, "Models: Build All"),
        PaletteEntry::new(MODELS_SHOW_DAG, "Models: Show DAG"),
        PaletteEntry::new(QUARTO_RENDER, "Quarto: Render"),
        PaletteEntry::new(DEBUG_SHOW_KEY_EVENTS, "Debug: Show Key Events"),
        PaletteEntry::new(DEBUG_SHOW_COMMAND_LOG, "Debug: Show Command Log"),
        PaletteEntry::new(DEBUG_SHOW_PANE_TREE, "Debug: Show Pane Tree"),
        PaletteEntry::new(DEBUG_SHOW_PROJECT_STATE, "Debug: Show Project State"),
        PaletteEntry::new(DEBUG_RELOAD_CONFIG, "Debug: Reload Config"),
        PaletteEntry::new(DEBUG_SHOW_STARTUP_TRACE, "Debug: Show Startup Trace"),
        PaletteEntry::new(APP_QUIT, "App: Quit"),
    ]
}

/// Paint the modal command-palette overlay on top of the workspace.
fn paint_palette(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    palette: &Palette,
    view_width: f32,
    view_height: f32,
) {
    // Dim the workspace behind the panel.
    canvas.rect(
        0.0,
        0.0,
        view_width,
        view_height,
        Color::rgba(0.0, 0.0, 0.0, 0.5),
    );

    let panel_w = 560.0_f32.min(view_width - 2.0 * PAD);
    let panel_x = ((view_width - panel_w) / 2.0).max(PAD);
    let panel_y = TOP_BAR_H + 40.0;
    let query_h = ROW_H + 6.0;
    let max_rows = 12usize;
    let shown = palette.matches().len().clamp(1, max_rows);
    let panel_h = query_h + shown as f32 * ROW_H + PAD;

    canvas.rect(panel_x, panel_y, panel_w, panel_h, theme.pane_background);
    canvas.rect(panel_x, panel_y, panel_w, 2.0, theme.accent);

    let (query_text, query_color) = if palette.query().is_empty() {
        ("> (type to filter commands)".to_string(), theme.muted_text)
    } else {
        (format!("> {}", palette.query()), theme.text)
    };
    canvas.text(panel_x + PAD, panel_y + 8.0, query_text, query_color, FONT);
    canvas.rect(
        panel_x,
        panel_y + query_h - 1.0,
        panel_w,
        1.0,
        theme.pane_border,
    );

    let rows_top = panel_y + query_h + PAD * 0.5;
    if palette.matches().is_empty() {
        canvas.text(
            panel_x + PAD,
            rows_top + 3.0,
            "No matching commands",
            theme.muted_text,
            FONT,
        );
        return;
    }

    for (row, palette_match) in palette.matches().iter().take(max_rows).enumerate() {
        let entry = &palette.entries()[palette_match.entry_index];
        let row_y = rows_top + row as f32 * ROW_H;
        let selected = row == palette.selection();
        if selected {
            canvas.rect(panel_x + 2.0, row_y, panel_w - 4.0, ROW_H, theme.selection);
        }
        let color = if selected {
            theme.text
        } else {
            theme.muted_text
        };
        canvas.text(panel_x + PAD, row_y + 3.0, entry.title.clone(), color, FONT);
    }
}

/// Paint the modal fuzzy-file-open overlay on top of the workspace.
fn paint_finder(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    finder: &FuzzyFinder,
    view_width: f32,
    view_height: f32,
) {
    canvas.rect(
        0.0,
        0.0,
        view_width,
        view_height,
        Color::rgba(0.0, 0.0, 0.0, 0.5),
    );

    let panel_w = 620.0_f32.min(view_width - 2.0 * PAD);
    let panel_x = ((view_width - panel_w) / 2.0).max(PAD);
    let panel_y = TOP_BAR_H + 40.0;
    let query_h = ROW_H + 6.0;
    let max_rows = 14usize;
    let shown = finder.matches().len().clamp(1, max_rows);
    let panel_h = query_h + shown as f32 * ROW_H + PAD;

    canvas.rect(panel_x, panel_y, panel_w, panel_h, theme.pane_background);
    canvas.rect(panel_x, panel_y, panel_w, 2.0, theme.accent);

    let (query_text, query_color) = if finder.query().is_empty() {
        ("> (type to filter files)".to_string(), theme.muted_text)
    } else {
        (format!("> {}", finder.query()), theme.text)
    };
    canvas.text(panel_x + PAD, panel_y + 8.0, query_text, query_color, FONT);
    canvas.rect(
        panel_x,
        panel_y + query_h - 1.0,
        panel_w,
        1.0,
        theme.pane_border,
    );

    let rows_top = panel_y + query_h + PAD * 0.5;
    if finder.matches().is_empty() {
        canvas.text(
            panel_x + PAD,
            rows_top + 3.0,
            "No matching files",
            theme.muted_text,
            FONT,
        );
        return;
    }

    for (row, fuzzy_match) in finder.matches().iter().take(max_rows).enumerate() {
        let path = &finder.items()[fuzzy_match.item_index];
        let row_y = rows_top + row as f32 * ROW_H;
        let selected = row == finder.selection();
        if selected {
            canvas.rect(panel_x + 2.0, row_y, panel_w - 4.0, ROW_H, theme.selection);
        }
        let color = if selected {
            theme.text
        } else {
            theme.muted_text
        };
        canvas.text(panel_x + PAD, row_y + 3.0, path.clone(), color, FONT);
    }
}

fn paint_completion(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    completion: &CompletionPopup,
    view_width: f32,
    view_height: f32,
) {
    let panel_w = 460.0_f32.min(view_width - 2.0 * PAD);
    let panel_x = (view_width * 0.5).min(view_width - panel_w - PAD).max(PAD);
    let panel_y = (view_height * 0.35).max(TOP_BAR_H + PAD);
    let max_rows = 8usize;
    let shown = completion.items().len().clamp(1, max_rows);
    let docs_h = completion
        .highlighted()
        .and_then(|item| item.documentation.as_ref().or(item.detail.as_ref()))
        .map(|_| ROW_H * 2.0)
        .unwrap_or(0.0);
    let panel_h = shown as f32 * ROW_H + docs_h + PAD;

    canvas.rect(panel_x, panel_y, panel_w, panel_h, theme.pane_background);
    canvas.rect(panel_x, panel_y, panel_w, 2.0, theme.accent);
    for (row, item) in completion.items().iter().take(max_rows).enumerate() {
        let row_y = panel_y + PAD * 0.5 + row as f32 * ROW_H;
        let selected = row == completion.selection();
        if selected {
            canvas.rect(panel_x + 2.0, row_y, panel_w - 4.0, ROW_H, theme.selection);
        }
        let color = if selected {
            theme.text
        } else {
            theme.muted_text
        };
        canvas.text(panel_x + PAD, row_y + 3.0, item.label.clone(), color, FONT);
    }
    if let Some(item) = completion.highlighted()
        && let Some(text) = item.documentation.as_ref().or(item.detail.as_ref())
    {
        let docs_y = panel_y + PAD * 0.5 + shown as f32 * ROW_H + PAD * 0.5;
        canvas.rect(panel_x, docs_y - 2.0, panel_w, 1.0, theme.pane_border);
        canvas.text(
            panel_x + PAD,
            docs_y + 3.0,
            text.lines().next().unwrap_or("").to_string(),
            theme.muted_text,
            FONT,
        );
    }
}

/// Paint the modal yes/no confirmation prompt (M4.4).
fn paint_confirm(
    canvas: &mut Canvas<'_>,
    theme: &Theme,
    prompt: &ConfirmPrompt,
    view_width: f32,
    view_height: f32,
) {
    canvas.rect(
        0.0,
        0.0,
        view_width,
        view_height,
        Color::rgba(0.0, 0.0, 0.0, 0.5),
    );
    let panel_w = 520.0_f32.min(view_width - 2.0 * PAD);
    let panel_x = ((view_width - panel_w) / 2.0).max(PAD);
    let body_lines: Vec<&str> = prompt.body().lines().collect();
    let panel_h = HEADER_H + ROW_H * (body_lines.len() as f32 + 2.0) + PAD * 2.0;
    let panel_y = ((view_height - panel_h) / 2.0).max(TOP_BAR_H + PAD);

    canvas.rect(panel_x, panel_y, panel_w, panel_h, theme.pane_background);
    canvas.rect(panel_x, panel_y, panel_w, 2.0, theme.accent);
    canvas.text(
        panel_x + PAD,
        panel_y + 6.0,
        prompt.title().to_string(),
        theme.text,
        FONT,
    );
    for (idx, line) in body_lines.iter().enumerate() {
        canvas.text(
            panel_x + PAD,
            panel_y + HEADER_H + idx as f32 * ROW_H,
            (*line).to_string(),
            theme.muted_text,
            FONT,
        );
    }
    let buttons_y = panel_y + HEADER_H + body_lines.len() as f32 * ROW_H + PAD;
    let button_w = 80.0;
    let yes_x = panel_x + panel_w - 2.0 * button_w - 2.0 * PAD;
    let no_x = panel_x + panel_w - button_w - PAD;
    let (yes_color, no_color) = if prompt.selection() {
        (theme.accent, theme.pane_border)
    } else {
        (theme.pane_border, theme.accent)
    };
    canvas.rect(yes_x, buttons_y, button_w, ROW_H, yes_color);
    canvas.rect(no_x, buttons_y, button_w, ROW_H, no_color);
    canvas.text(
        yes_x + PAD,
        buttons_y + 3.0,
        "[y] Yes".to_string(),
        theme.text,
        FONT,
    );
    canvas.text(
        no_x + PAD,
        buttons_y + 3.0,
        "[n] No".to_string(),
        theme.text,
        FONT,
    );
}

/// [`CockpitApp`] adapter: holds the project launcher, a hydrating
/// cold-start driver, or a live [`AppModel`] and forwards harness
/// callbacks accordingly.
///
/// The shell is the **single** `CockpitApp` implementation the binary
/// hands to [`cockpit_render::run_app`] (M7.1: `run_app` is called at
/// most once per process — `winit::EventLoop` cannot be recreated). It
/// transitions in-place between its states:
///
/// - `Launcher` → the project picker (recent projects, Open Folder).
///   On selection, [`tick`](CockpitApp::tick) replaces this state with
///   `Hydrating` for the chosen path.
/// - `Hydrating` → splash painted on frame 1, one cold-start phase
///   advanced per frame (v0.6 M6.2). On completion, transitions to
///   `Live(model)`.
/// - `Live` → the three-pane cockpit. Every harness callback forwards
///   to the model.
/// - `Failed` → splash stays up with the hydration error message.
pub struct AppShell {
    state: ShellState,
    /// Splash / launcher theme. Kept on the shell so [`CockpitApp::theme`]
    /// can hand back a `&Theme` while no model exists yet.
    splash_theme: Theme,
    /// Redraw handle stashed before the model is born so we can hand it
    /// to the live model the moment hydration completes (background PTY
    /// threads expect a wake handle).
    pending_redraw: Option<RedrawHandle>,
    /// Once the launcher signals `Exit`, the shell flips this on and
    /// reports it to the harness via [`CockpitApp::wants_exit`].
    exit_requested: bool,
}

/// Lifecycle of the shell.
pub(crate) enum ShellState {
    /// Project picker; user hasn't chosen a project yet (M7.1).
    Launcher(crate::launcher::LauncherModel),
    /// Cold-start phases still running. Painter shows the splash.
    Hydrating(crate::hydration::HydrationDriver),
    /// Live cockpit. Every harness callback forwards to the model.
    Live(AppModel),
    /// Hydration failed; the splash stays on screen with the error.
    /// Holds the final progress snapshot so the splash can re-render.
    Failed(cockpit_ui::HydrationProgress),
    /// Transient placeholder used by `mem::replace` during state
    /// transitions. Never observable from outside the shell.
    Transitioning,
}

impl AppShell {
    /// Build a shell that opens the project launcher (M7.1). When the
    /// user picks a project, the shell transitions to hydrating inside
    /// the same event loop — no second `run_app` call.
    pub fn launcher(model: crate::launcher::LauncherModel) -> Self {
        Self {
            state: ShellState::Launcher(model),
            splash_theme: Theme::default(),
            pending_redraw: None,
            exit_requested: false,
        }
    }

    /// Build a shell that will hydrate the project at `path` on the
    /// render thread (M6.2). The window opens with the splash painted on
    /// frame 1; subsequent frames run one cold-start phase each.
    pub fn hydrating(path: std::path::PathBuf) -> Self {
        Self {
            state: ShellState::Hydrating(crate::hydration::HydrationDriver::new(path)),
            splash_theme: Theme::default(),
            pending_redraw: None,
            exit_requested: false,
        }
    }

    // The predicates below are introspection helpers used by tests to
    // drive `tick`-based state-machine assertions without exposing the
    // private `ShellState` enum. They have no production callers;
    // clippy's dead-code lint doesn't follow `cfg(test)`-gated callers
    // from sibling modules, so we mark the helpers explicitly.
    #[allow(dead_code)]
    pub fn is_launcher(&self) -> bool {
        matches!(self.state, ShellState::Launcher(_))
    }

    #[allow(dead_code)]
    pub fn is_hydrating(&self) -> bool {
        matches!(self.state, ShellState::Hydrating(_))
    }

    #[allow(dead_code)]
    pub fn is_live(&self) -> bool {
        matches!(self.state, ShellState::Live(_))
    }

    #[allow(dead_code)]
    pub fn is_failed(&self) -> bool {
        matches!(self.state, ShellState::Failed(_))
    }

    #[allow(dead_code)]
    pub fn model(&self) -> Option<&AppModel> {
        match &self.state {
            ShellState::Live(model) => Some(model),
            _ => None,
        }
    }
}

impl CockpitApp for AppShell {
    fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
        match &mut self.state {
            ShellState::Launcher(model) => model.paint(painter, viewport),
            ShellState::Live(model) => model.paint(painter, viewport),
            ShellState::Hydrating(driver) => {
                crate::splash::paint_splash(
                    painter,
                    viewport,
                    driver.progress(),
                    &self.splash_theme,
                );
            }
            ShellState::Failed(progress) => {
                crate::splash::paint_splash(painter, viewport, progress, &self.splash_theme);
            }
            ShellState::Transitioning => {
                // Defensive: the splash background keeps a frame from
                // flashing through if we ever observe Transitioning.
                crate::splash::paint_splash(
                    painter,
                    viewport,
                    &cockpit_ui::HydrationProgress::default_phases(),
                    &self.splash_theme,
                );
            }
        }
    }

    fn theme(&self) -> &Theme {
        match &self.state {
            ShellState::Launcher(model) => model.theme(),
            ShellState::Live(model) => &model.theme,
            _ => &self.splash_theme,
        }
    }

    fn on_key(&mut self, chord: KeyChord) {
        match &mut self.state {
            ShellState::Launcher(model) => model.on_key(chord),
            ShellState::Live(model) => model.dispatch(chord),
            _ => {
                // Splash and failed states ignore input — there's
                // nothing meaningful for the user to do except wait or
                // close the window. The OS close affordance still works.
            }
        }
    }

    fn on_mouse_down(&mut self, button: MouseButton, position: PointerPosition) {
        if let ShellState::Live(model) = &mut self.state {
            model.on_pointer_down(button, position);
        }
    }

    fn on_mouse_up(&mut self, button: MouseButton, position: PointerPosition) {
        if let ShellState::Live(model) = &mut self.state {
            model.on_pointer_up(button, position);
        }
    }

    fn on_mouse_move(&mut self, position: PointerPosition) {
        if let ShellState::Live(model) = &mut self.state {
            model.on_pointer_move(position);
        }
    }

    fn on_scroll(&mut self, position: PointerPosition, dx: f32, dy: f32) {
        if let ShellState::Live(model) = &mut self.state {
            model.on_scroll(position, dx, dy);
        }
    }

    fn set_redraw_handle(&mut self, handle: RedrawHandle) {
        match &mut self.state {
            ShellState::Live(model) => model.set_redraw_handle(handle),
            _ => {
                // The model doesn't exist yet — stash the handle so it
                // is wired up the moment hydration completes.
                self.pending_redraw = Some(handle);
            }
        }
    }

    fn tick(&mut self) {
        match &mut self.state {
            ShellState::Launcher(model) => {
                // M7.1 hand-off: a launcher selection becomes a state
                // transition in-place. No second `run_app` — the same
                // event loop now drives hydration of the chosen project.
                match model.result() {
                    Some(crate::launcher::LauncherResult::OpenProject(path)) => {
                        tracing::info!(path = %path.display(), "launcher selected project");
                        self.state =
                            ShellState::Hydrating(crate::hydration::HydrationDriver::new(path));
                    }
                    Some(crate::launcher::LauncherResult::Exit) => {
                        self.exit_requested = true;
                    }
                    None => {}
                }
            }
            ShellState::Hydrating(driver) => match driver.advance() {
                crate::hydration::HydrationOutcome::Continue => {}
                crate::hydration::HydrationOutcome::Ready(model) => {
                    let mut model = *model;
                    if let Some(handle) = self.pending_redraw.take() {
                        model.set_redraw_handle(handle);
                    }
                    tracing::info!(project = model.project_name(), "cockpit hydration complete");
                    self.state = ShellState::Live(model);
                }
                crate::hydration::HydrationOutcome::Failed(message) => {
                    tracing::error!(error = %message, "cockpit hydration failed");
                    let prev = std::mem::replace(&mut self.state, ShellState::Transitioning);
                    let progress = match prev {
                        ShellState::Hydrating(driver) => driver.progress().clone(),
                        _ => cockpit_ui::HydrationProgress::default_phases(),
                    };
                    self.state = ShellState::Failed(progress);
                }
            },
            _ => {}
        }
    }

    fn wants_continuous_redraw(&self) -> bool {
        // Launcher needs continuous redraws so `tick()` can spot the
        // result the frame after the user presses Enter (otherwise the
        // hand-off would wait for the next input event).
        matches!(
            self.state,
            ShellState::Hydrating(_) | ShellState::Launcher(_)
        )
    }

    fn on_shutdown(&mut self) {
        if let ShellState::Live(model) = &mut self.state {
            model.on_shutdown();
        }
    }

    fn wants_exit(&self) -> bool {
        if self.exit_requested {
            return true;
        }
        match &self.state {
            ShellState::Live(model) => model.wants_exit(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::detect_project;
    use cockpit_testkit::fixture_path;

    fn model() -> AppModel {
        let path = fixture_path("file-tree");
        let detection = detect_project(&path).expect("detect file-tree fixture");
        let tree = FileTree::load(&path).expect("load file-tree fixture");
        AppModel::new(detection, tree).expect("build model")
    }

    /// A model over the `mise-basic` fixture, which has `lint` and `test` tasks.
    fn mise_model() -> AppModel {
        let path = fixture_path("mise-basic");
        let detection = detect_project(&path).expect("detect mise-basic fixture");
        let tree = FileTree::load(&path).expect("load mise-basic fixture");
        AppModel::new(detection, tree).expect("build model")
    }

    fn chord(input: &str) -> KeyChord {
        input.parse().expect("valid chord")
    }

    fn type_keys(model: &mut AppModel, keys: &str) {
        for key in keys.chars() {
            let chord = if key == ' ' {
                chord("Space")
            } else {
                chord(&key.to_string())
            };
            model.dispatch(chord);
        }
    }

    /// A model over the `rust-basic` fixture, which has `src/main.rs`.
    fn rust_model() -> AppModel {
        let path = fixture_path("rust-basic");
        let detection = detect_project(&path).expect("detect rust-basic fixture");
        let tree = FileTree::load(&path).expect("load rust-basic fixture");
        AppModel::new(detection, tree).expect("build model")
    }

    #[test]
    fn open_path_reference_jumps_to_a_file_at_line_and_column() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs:2:5");

        let doc = model.document.as_ref().expect("document opened");
        assert_eq!(doc.name, "main.rs");
        assert_eq!(
            doc.editor.cursor().line_col(doc.editor.buffer()),
            (1, 4),
            "cursor lands at the 1-based line:col, converted to 0-based"
        );
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn open_path_reference_reports_a_missing_file() {
        let mut model = rust_model();
        model.open_path_reference("src/nope.rs:1");
        assert!(model.document.is_none());
        assert!(
            model.status.contains("not found"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn open_terminal_paths_without_a_terminal_reports_it() {
        let mut model = rust_model();
        model.open_terminal_paths();
        assert!(model.document.is_none());
        assert!(
            model.status.contains("No terminal"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn send_file_path_without_a_document_reports_it() {
        let mut model = rust_model();
        model.send_file_path_to_terminal();
        assert!(
            model.status.contains("No file open"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn send_selection_without_a_document_reports_it() {
        let mut model = rust_model();
        model.send_selection_to_terminal();
        assert!(
            model.status.contains("No file open"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn send_selection_without_a_visual_selection_reports_it() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");
        assert!(model.document.is_some(), "fixture file should open");

        model.send_selection_to_terminal();
        assert!(
            model.status.contains("No selection"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn run_test_all_requires_a_test_mise_task() {
        let mut model = rust_model();
        model.run_test_all();
        assert!(
            model.status.contains("No `test` mise task"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn run_test_current_file_requires_a_document() {
        let mut model = mise_model();
        model.run_test_current_file();
        assert!(
            model.status.contains("No file open"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn run_test_nearest_requires_a_document() {
        let mut model = mise_model();
        model.run_test_nearest();
        assert!(
            model.status.contains("No file open"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn dispatch_records_each_chord_in_the_key_log() {
        let mut model = model();
        model.dispatch(chord("j"));
        model.dispatch(chord("k"));
        model.dispatch(chord("Ctrl+h"));
        let recent: Vec<&str> = model.key_log.iter().map(String::as_str).collect();
        assert_eq!(recent, vec!["j", "k", "Ctrl+h"]);
    }

    #[test]
    fn key_log_is_a_bounded_ring_buffer() {
        let mut model = model();
        for _ in 0..(DEBUG_LOG_SIZE + 5) {
            model.dispatch(chord("j"));
        }
        assert_eq!(model.key_log.len(), DEBUG_LOG_SIZE);
    }

    #[test]
    fn debug_show_key_events_summarises_the_ring_buffer() {
        let mut model = model();
        model.dispatch(chord("j"));
        model.dispatch(chord("k"));
        model.debug_show_key_events();
        assert!(
            model.status.contains("Key events: j, k"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn debug_show_project_state_includes_signals_and_mise() {
        let mut model = mise_model();
        model.debug_show_project_state();
        assert!(
            model.status.contains("mise-basic"),
            "status: {}",
            model.status,
        );
        assert!(model.status.contains("mise["), "status: {}", model.status,);
    }

    #[test]
    fn debug_reload_config_restores_default_keybindings() {
        let mut model = model();
        model.debug_reload_config();
        assert!(
            model.status.starts_with("Config reloaded"),
            "status: {}",
            model.status,
        );
    }

    #[test]
    fn file_uri_and_path_round_trip() {
        let path = PathBuf::from(if cfg!(windows) {
            "C:\\code\\proj\\src\\main.rs"
        } else {
            "/code/proj/src/main.rs"
        });
        let uri = file_uri(&path);
        assert_eq!(path_from_file_uri(&uri), Some(path));
    }

    #[test]
    fn apply_publish_diagnostics_stores_diagnostics_for_path() {
        let mut model = rust_model();
        let path = if cfg!(windows) {
            "C:/code/main.rs"
        } else {
            "/code/main.rs"
        };
        let params: PublishDiagnosticsParams = serde_json::from_value(serde_json::json!({
            "uri": format!("file://{}{}", if cfg!(windows) { "/" } else { "" }, path),
            "diagnostics": [
                {
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}},
                    "severity": 1,
                    "message": "expected `;`"
                },
                {
                    "range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 8}},
                    "severity": 2,
                    "message": "unused variable"
                }
            ]
        }))
        .unwrap();

        model.apply_publish_diagnostics(params);

        let expected = PathBuf::from(if cfg!(windows) {
            "C:\\code\\main.rs"
        } else {
            "/code/main.rs"
        });
        let stored = model.diagnostics_for(&expected);
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].message, "expected `;`");
        assert_eq!(stored[1].message, "unused variable");
    }

    #[test]
    fn apply_publish_diagnostics_clears_on_empty_list() {
        let mut model = rust_model();
        let uri_prefix = if cfg!(windows) { "file:///" } else { "file://" };
        let path_str = if cfg!(windows) { "C:/x.rs" } else { "/x.rs" };
        let with_diags: PublishDiagnosticsParams = serde_json::from_value(serde_json::json!({
            "uri": format!("{uri_prefix}{path_str}"),
            "diagnostics": [{
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "message": "bad"
            }]
        }))
        .unwrap();
        model.apply_publish_diagnostics(with_diags);
        let expected = PathBuf::from(if cfg!(windows) { "C:\\x.rs" } else { "/x.rs" });
        assert_eq!(model.diagnostics_for(&expected).len(), 1);

        let cleared: PublishDiagnosticsParams = serde_json::from_value(serde_json::json!({
            "uri": format!("{uri_prefix}{path_str}"),
            "diagnostics": []
        }))
        .unwrap();
        model.apply_publish_diagnostics(cleared);
        assert!(model.diagnostics_for(&expected).is_empty());
    }

    #[test]
    fn parse_first_location_reads_a_single_location() {
        let v = serde_json::json!({
            "uri": "file:///code/main.rs",
            "range": {
                "start": {"line": 12, "character": 4},
                "end":   {"line": 12, "character": 9},
            }
        });
        let (uri, line, col) = parse_first_location(&v).expect("single Location parses");
        assert_eq!(uri, "file:///code/main.rs");
        assert_eq!((line, col), (12, 4));
    }

    #[test]
    fn parse_first_location_reads_a_location_link() {
        let v = serde_json::json!([{
            "targetUri": "file:///code/lib.rs",
            "targetSelectionRange": {
                "start": {"line": 7, "character": 0},
                "end":   {"line": 7, "character": 3},
            },
            "targetRange": {
                "start": {"line": 5, "character": 0},
                "end":   {"line": 9, "character": 0},
            }
        }]);
        let (uri, line, col) = parse_first_location(&v).expect("LocationLink array parses");
        assert_eq!(uri, "file:///code/lib.rs");
        // targetSelectionRange wins over targetRange (more precise).
        assert_eq!((line, col), (7, 0));
    }

    #[test]
    fn parse_first_location_skips_an_empty_array() {
        assert!(parse_first_location(&serde_json::json!([])).is_none());
        assert!(parse_first_location(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn extract_hover_contents_handles_markup_content() {
        let v = serde_json::json!({
            "contents": {"kind": "markdown", "value": "**fn foo()**\n\nDoes a thing."}
        });
        assert_eq!(
            extract_hover_contents(&v).as_deref(),
            Some("**fn foo()**\n\nDoes a thing."),
        );
    }

    #[test]
    fn extract_hover_contents_handles_marked_string_array() {
        let v = serde_json::json!({
            "contents": [
                {"language": "rust", "value": "fn foo()"},
                "Bare line",
            ]
        });
        assert_eq!(
            extract_hover_contents(&v).as_deref(),
            Some("fn foo()\n\nBare line"),
        );
    }

    #[test]
    fn extract_hover_contents_returns_none_for_empty() {
        assert!(extract_hover_contents(&serde_json::Value::Null).is_none());
        assert!(
            extract_hover_contents(&serde_json::json!({"contents": ""})).is_none(),
            "empty string hover yields no info",
        );
    }

    #[test]
    fn apply_goto_definition_result_opens_the_target_file_at_line_and_column() {
        let mut model = rust_model();
        let target = model.detection.root_path.join("src").join("main.rs");
        let uri = file_uri(&target);
        let result = serde_json::json!({
            "uri": uri,
            "range": {
                "start": {"line": 1, "character": 4},
                "end":   {"line": 1, "character": 7},
            }
        });

        model.apply_goto_definition_result(result);

        let doc = model.document.as_ref().expect("document opened");
        assert_eq!(doc.name, "main.rs");
        // LSP 0-based (1,4) → editor's 0-based (1,4) once the +1/−1
        // round-trip through `open_document_at` settles.
        assert_eq!(doc.editor.cursor().line_col(doc.editor.buffer()), (1, 4));
    }

    #[test]
    fn apply_goto_definition_result_reports_when_no_definition() {
        let mut model = rust_model();
        model.apply_goto_definition_result(serde_json::Value::Null);
        assert_eq!(model.status, "No definition found.");
        assert!(model.document.is_none());
    }

    #[test]
    fn apply_hover_result_stores_contents_and_updates_status() {
        let mut model = rust_model();
        let path = model.detection.root_path.join("src").join("main.rs");
        let result = serde_json::json!({
            "contents": {"kind": "markdown", "value": "fn main()\n\nEntry point."}
        });

        model.apply_hover_result(path.clone(), result);

        let hover = model.hover.as_ref().expect("hover stored");
        assert_eq!(hover.path, path);
        assert!(hover.contents.starts_with("fn main()"));
        assert_eq!(model.status, "Hover: fn main()");
    }

    #[test]
    fn apply_hover_result_clears_hover_on_empty_response() {
        let mut model = rust_model();
        // Seed an old hover so we can prove it gets cleared.
        model.hover = Some(HoverInfo {
            path: PathBuf::from("/tmp/x.rs"),
            contents: "stale".to_string(),
        });
        model.apply_hover_result(PathBuf::from("/tmp/x.rs"), serde_json::Value::Null);
        assert!(model.hover.is_none());
        assert_eq!(model.status, "No hover information.");
    }

    #[test]
    fn apply_workspace_edit_updates_the_open_document() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");
        let path = model.document.as_ref().unwrap().path.clone();
        let edit = serde_json::json!({
            "changes": {
                file_uri(&path): [{
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 2}
                    },
                    "newText": "pub fn"
                }]
            }
        });

        let count = model.apply_workspace_edit(&edit).expect("edit applies");

        assert_eq!(count, 1);
        let doc = model.document.as_ref().unwrap();
        assert!(doc.editor.text().starts_with("pub fn"));
        assert!(doc.editor.is_dirty());
    }

    #[test]
    fn apply_code_action_result_applies_first_edit() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");
        let path = model.document.as_ref().unwrap().path.clone();
        let result = serde_json::json!([{
            "title": "Make function public",
            "edit": {
                "changes": {
                    file_uri(&path): [{
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0}
                        },
                        "newText": "pub "
                    }]
                }
            }
        }]);

        model.apply_code_action_result(result);

        assert!(
            model
                .document
                .as_ref()
                .unwrap()
                .editor
                .text()
                .starts_with("pub ")
        );
        assert!(model.status.contains("Make function public"));
    }

    #[test]
    fn open_rename_input_seeds_symbol_under_cursor() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs:1:4");

        model.open_rename_input();

        let input = model.rename_input.as_ref().expect("rename input open");
        assert_eq!(input.value, "main");
        assert!(model.status.contains("main"));
    }

    #[test]
    fn rename_input_edits_and_escape_cancels() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs:1:4");
        model.open_rename_input();

        model.handle_rename_key(&chord("Backspace"));
        model.handle_rename_key(&chord("x"));
        assert!(model.rename_input.as_ref().unwrap().value.ends_with('x'));

        model.handle_rename_key(&chord("Escape"));
        assert!(model.rename_input.is_none());
        assert_eq!(model.status, "Rename cancelled.");
    }

    #[test]
    fn apply_rename_result_applies_workspace_edit() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");
        let path = model.document.as_ref().unwrap().path.clone();
        let result = serde_json::json!({
            "changes": {
                file_uri(&path): [{
                    "range": {
                        "start": {"line": 0, "character": 3},
                        "end": {"line": 0, "character": 7}
                    },
                    "newText": "entry"
                }]
            }
        });

        model.apply_rename_result(result);

        assert!(
            model
                .document
                .as_ref()
                .unwrap()
                .editor
                .text()
                .contains("entry")
        );
        assert!(model.status.contains("Rename applied"));
    }

    #[test]
    fn apply_completion_result_opens_popup_with_docs() {
        let mut model = rust_model();
        let result = serde_json::json!({
            "isIncomplete": false,
            "items": [{
                "label": "println!",
                "detail": "macro",
                "documentation": {"kind": "markdown", "value": "Prints a line."},
                "insertText": "println!($0)"
            }]
        });

        model.apply_completion_result(result);

        let completion = model.completion.as_ref().expect("completion popup");
        let item = completion.highlighted().unwrap();
        assert_eq!(item.label, "println!");
        assert_eq!(item.insert_text(), "println!($0)");
        assert_eq!(item.documentation.as_deref(), Some("Prints a line."));
    }

    #[test]
    fn accepting_completion_inserts_selected_text() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs:1:1");
        model.completion = Some(CompletionPopup::new(vec![CompletionItem {
            label: "main".to_string(),
            detail: None,
            documentation: None,
            insert_text: Some("main()".to_string()),
        }]));

        model.handle_completion_key(&chord("Enter"));

        assert!(model.completion.is_none());
        assert!(
            model
                .document
                .as_ref()
                .unwrap()
                .editor
                .text()
                .starts_with("main()")
        );
    }

    #[test]
    fn request_code_action_requires_a_current_diagnostic() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.request_code_action();

        assert_eq!(model.status, "No diagnostic at the cursor.");
    }

    #[test]
    fn editor_k_dispatches_show_hover_command() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.handle_editor_key(&chord("K"));

        assert_eq!(
            model.command_log.back().map(String::as_str),
            Some(LSP_SHOW_HOVER)
        );
    }

    #[test]
    fn editor_gd_dispatches_goto_definition_command() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.handle_editor_key(&chord("g"));
        model.handle_editor_key(&chord("d"));

        assert_eq!(
            model.command_log.back().map(String::as_str),
            Some(LSP_GOTO_DEFINITION)
        );
    }

    #[test]
    fn editor_leader_ca_dispatches_code_action_command() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.handle_editor_key(&chord("Space"));
        model.handle_editor_key(&chord("c"));
        model.handle_editor_key(&chord("a"));

        assert_eq!(
            model.command_log.back().map(String::as_str),
            Some(LSP_CODE_ACTION)
        );
    }

    #[test]
    fn editor_leader_cr_dispatches_rename_command() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.handle_editor_key(&chord("Space"));
        model.handle_editor_key(&chord("c"));
        model.handle_editor_key(&chord("r"));

        assert_eq!(
            model.command_log.back().map(String::as_str),
            Some(LSP_RENAME)
        );
        assert!(model.rename_input.is_some());
    }

    #[test]
    fn ctrl_space_dispatches_completion_command() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs");

        model.handle_editor_key(&chord("Ctrl+Space"));

        assert_eq!(
            model.command_log.back().map(String::as_str),
            Some(LSP_COMPLETION)
        );
    }

    #[test]
    fn editor_gg_still_reaches_vim() {
        let mut model = rust_model();
        model.open_path_reference("src/main.rs:2:1");

        model.handle_editor_key(&chord("g"));
        model.handle_editor_key(&chord("g"));

        let doc = model.document.as_ref().expect("document remains open");
        assert_eq!(doc.editor.cursor().line_col(doc.editor.buffer()).0, 0);
    }

    #[test]
    fn request_goto_definition_without_a_document_reports_it() {
        let mut model = rust_model();
        model.request_goto_definition();
        assert_eq!(model.status, "No document open.");
        assert!(model.lsp_pending.is_empty());
    }

    #[test]
    fn lsp_servers_launch_via_mise_exec() {
        let config = ServerConfig::for_language(Language::Rust).expect("rust-analyzer config");
        let argv = lsp_launch_argv(&config);
        assert_eq!(&argv[..3], &["mise", "exec", "--"]);
        assert_eq!(argv[3], "rust-analyzer");
        assert_eq!(argv.len(), 4, "no extra args today: argv was {argv:?}",);
    }

    #[test]
    fn diagnostic_color_picks_per_severity() {
        let model = rust_model();
        assert_eq!(
            model.diagnostic_color(Some(DiagnosticSeverity::ERROR)),
            model.theme.diagnostic_error,
        );
        assert_eq!(
            model.diagnostic_color(Some(DiagnosticSeverity::WARNING)),
            model.theme.diagnostic_warning,
        );
        assert_eq!(
            model.diagnostic_color(Some(DiagnosticSeverity::INFORMATION)),
            model.theme.diagnostic_info,
        );
        assert_eq!(
            model.diagnostic_color(Some(DiagnosticSeverity::HINT)),
            model.theme.diagnostic_hint,
        );
        // Missing severity defaults to error (matching VS Code / rust-analyzer).
        assert_eq!(model.diagnostic_color(None), model.theme.diagnostic_error,);
    }

    #[test]
    fn focus_commands_move_the_focused_pane() {
        let mut model = model();
        assert_eq!(model.layout.focused(), PaneId::Editor);

        model.dispatch(chord("Ctrl+h"));
        assert_eq!(model.layout.focused(), PaneId::Files);

        model.dispatch(chord("Ctrl+l"));
        assert_eq!(model.layout.focused(), PaneId::Terminal);

        model.dispatch(chord("Ctrl+j"));
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn toggle_terminal_hides_and_shows_the_pane() {
        let mut model = model();
        assert!(model.layout.preferences().terminal_visible);

        model.dispatch(chord("Ctrl+`"));
        assert!(!model.layout.preferences().terminal_visible);

        model.dispatch(chord("Ctrl+`"));
        assert!(model.layout.preferences().terminal_visible);
    }

    #[test]
    fn file_pane_navigation_moves_the_selection() {
        let mut model = model();
        model.dispatch(chord("Ctrl+h"));
        assert_eq!(model.browser.selected_index(), 0);

        model.dispatch(chord("j"));
        assert_eq!(model.browser.selected().unwrap().name, "tests");

        model.dispatch(chord("k"));
        assert_eq!(model.browser.selected().unwrap().name, "src");
    }

    #[test]
    fn enter_opens_a_file_into_the_editor() {
        let mut model = model();
        model.dispatch(chord("Ctrl+h"));
        model.dispatch(chord("Enter")); // expand src
        model.dispatch(chord("j")); // nested
        model.dispatch(chord("j")); // lib.rs
        model.dispatch(chord("Enter"));

        assert!(model.document.is_some());
        assert_eq!(model.layout.focused(), PaneId::Editor);
        assert_eq!(model.document.as_ref().unwrap().name, "lib.rs");
    }

    #[test]
    fn editor_keys_drive_the_vim_state_machine() {
        let mut model = open_temp_doc("abc").0;
        // `x` in Normal mode deletes the first character.
        model.dispatch(chord("x"));
        assert_eq!(model.document.as_ref().unwrap().editor.text(), "bc");
    }

    #[test]
    fn ctrl_q_requests_exit() {
        let mut model = model();
        assert!(!model.wants_exit());
        model.dispatch(chord("Ctrl+q"));
        assert!(model.wants_exit());
    }

    #[test]
    fn paint_emits_draw_commands() {
        let mut model = model();
        let mut painter = Painter::new();
        model.paint(
            &mut painter,
            Viewport {
                width: 1280,
                height: 800,
                scale: 1.0,
            },
        );
        assert!(
            !painter.commands().is_empty(),
            "paint produced no draw commands"
        );
    }

    #[test]
    fn paint_terminal_mux_split_emits_pane_labels() {
        let mut model = model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);
        let mut painter = Painter::new();

        model.paint(
            &mut painter,
            Viewport {
                width: 1280,
                height: 800,
                scale: 1.0,
            },
        );

        let labels: Vec<&str> = painter
            .commands()
            .iter()
            .filter_map(|command| match command {
                cockpit_render::DrawCommand::Text(run) => Some(run.text.as_str()),
                _ => None,
            })
            .collect();
        assert!(labels.contains(&"pane-0"));
        assert!(labels.contains(&"pane-1"));
    }

    /// Build a model whose editor already has `contents` open on a temp file.
    fn open_temp_doc(contents: &str) -> (AppModel, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("note.txt");
        std::fs::write(&file, contents).expect("seed file");

        let detection = detect_project(dir.path()).expect("detect temp project");
        let tree = FileTree::load(dir.path()).expect("load temp tree");
        let mut model = AppModel::new(detection, tree).expect("build model");

        model.dispatch(chord("Ctrl+h"));
        model.dispatch(chord("Enter")); // open note.txt
        assert!(model.document.is_some(), "note.txt should be open");
        (model, dir)
    }

    #[test]
    fn edit_with_vim_keys_then_save_writes_the_file() {
        let (mut model, dir) = open_temp_doc("hello");

        // Insert "X " at the start, then leave Insert mode.
        model.dispatch(chord("i"));
        type_keys(&mut model, "X ");
        model.dispatch(chord("Escape"));
        assert!(model.document.as_ref().unwrap().editor.is_dirty());

        // `:w` writes the buffer back to disk.
        model.dispatch(chord(":"));
        type_keys(&mut model, "w");
        model.dispatch(chord("Enter"));

        let written = std::fs::read_to_string(dir.path().join("note.txt")).expect("read back");
        assert_eq!(written, "X hello");
        assert!(!model.document.as_ref().unwrap().editor.is_dirty());
    }

    #[test]
    fn ctrl_s_saves_the_open_document() {
        let (mut model, dir) = open_temp_doc("data");
        model.dispatch(chord("i"));
        type_keys(&mut model, "!");
        model.dispatch(chord("Escape"));
        model.dispatch(chord("Ctrl+s"));

        let written = std::fs::read_to_string(dir.path().join("note.txt")).expect("read back");
        assert_eq!(written, "!data");
    }

    #[test]
    fn ctrl_shift_p_opens_the_command_palette() {
        let mut model = model();
        assert!(model.palette.is_none());
        model.dispatch(chord("Ctrl+Shift+p"));
        assert!(model.palette.is_some());
        assert_eq!(
            model.palette.as_ref().unwrap().matches().len(),
            palette_entries().len()
        );
    }

    #[test]
    fn typing_in_the_palette_filters_commands() {
        let mut model = model();
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "quit");

        let palette = model.palette.as_ref().unwrap();
        assert_eq!(
            palette.highlighted().unwrap().id,
            cockpit_commands::CommandId::from(APP_QUIT)
        );
    }

    #[test]
    fn escape_closes_the_palette_without_running_a_command() {
        let mut model = model();
        model.dispatch(chord("Ctrl+Shift+p"));
        model.dispatch(chord("Escape"));
        assert!(model.palette.is_none());
        assert!(!model.wants_exit());
    }

    #[test]
    fn palette_enter_dispatches_the_highlighted_command() {
        let mut model = model();
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "quit");
        model.dispatch(chord("Enter"));

        assert!(model.palette.is_none());
        assert!(model.wants_exit());
    }

    #[test]
    fn palette_arrows_move_the_selection() {
        let mut model = model();
        model.dispatch(chord("Ctrl+Shift+p"));
        assert_eq!(model.palette.as_ref().unwrap().selection(), 0);

        model.dispatch(chord("ArrowDown"));
        model.dispatch(chord("ArrowDown"));
        assert_eq!(model.palette.as_ref().unwrap().selection(), 2);

        model.dispatch(chord("ArrowUp"));
        assert_eq!(model.palette.as_ref().unwrap().selection(), 1);
    }

    #[test]
    fn palette_focus_command_changes_the_active_pane() {
        let mut model = model();
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "focus files");
        model.dispatch(chord("Enter"));

        assert!(model.palette.is_none());
        assert_eq!(model.layout.focused(), PaneId::Files);
    }

    #[test]
    fn focusing_the_terminal_without_a_redraw_handle_is_a_safe_no_op() {
        let mut model = model();
        model.dispatch(chord("Ctrl+l"));
        assert_eq!(model.layout.focused(), PaneId::Terminal);
        // No redraw handle was set, so no PTY is spawned — and nothing panics.
        assert!(model.terminals.is_empty());
    }

    #[test]
    fn mux_prefix_split_updates_the_headless_layout_tree() {
        let mut model = model();
        model.dispatch(chord("Ctrl+l"));

        model.dispatch(chord("Ctrl+b"));
        model.dispatch(chord("%"));

        let window = model.mux_session.active_window();
        assert_eq!(window.layout.leaves().len(), 2);
        assert_eq!(window.active.get(), 1);
        assert_eq!(model.layout.focused(), PaneId::Terminal);
        assert!(model.status.contains("split terminal pane"));
        assert!(
            model
                .recent_commands_summary()
                .contains(mux_command_ids::SPLIT_HORIZONTAL)
        );
    }

    #[test]
    fn mux_prefix_ctrl_arrow_resizes_the_active_split() {
        let mut model = model();
        model.dispatch(chord("Ctrl+l"));
        model.dispatch(chord("Ctrl+b"));
        model.dispatch(chord("%"));

        model.dispatch(chord("Ctrl+b"));
        model.dispatch(chord("Ctrl+ArrowRight"));

        match &model.mux_session.active_window().layout {
            cockpit_mux::LayoutNode::Split { ratio, .. } => assert!((*ratio - 0.45).abs() < 0.001),
            other => panic!("expected split layout, got {other:?}"),
        }
        assert!(
            model
                .recent_commands_summary()
                .contains(mux_command_ids::RESIZE_RIGHT)
        );
        assert!(model.status.contains("resized pane-1"));
    }

    #[test]
    fn mux_next_layout_cycles_preset_state() {
        let mut model = model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);
        model.run_command(mux_command_ids::NEXT_LAYOUT);
        assert_eq!(
            model.mux_session.active_window().layout_preset,
            cockpit_mux::LayoutPreset::MainVertical
        );

        model.run_command(mux_command_ids::NEXT_LAYOUT);
        assert_eq!(
            model.mux_session.active_window().layout_preset,
            cockpit_mux::LayoutPreset::Tiled
        );
    }

    #[test]
    fn mux_prefix_consumes_unknown_followup_without_forwarding_to_terminal() {
        let mut model = model();
        model.dispatch(chord("Ctrl+l"));

        model.dispatch(chord("Ctrl+b"));
        model.dispatch(chord("q"));

        assert!(!model.wants_exit());
        assert_eq!(model.mux_session.active_window().layout.leaves().len(), 1);
        assert_eq!(
            model.mux_prefix.state(),
            cockpit_mux::PrefixState::Passthrough
        );
    }

    #[test]
    fn mux_palette_commands_share_the_same_command_path() {
        let mut model = model();
        model.run_command(mux_command_ids::NEW_WINDOW);
        model.run_command(mux_command_ids::SELECT_WINDOW_0);

        assert_eq!(model.mux_session.windows.len(), 2);
        assert_eq!(model.mux_session.active.get(), 0);
        assert!(
            model
                .recent_commands_summary()
                .contains(mux_command_ids::SELECT_WINDOW_0)
        );
    }

    #[test]
    fn palette_lists_every_mux_resize_command() {
        let ids = palette_entries()
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect::<Vec<_>>();

        for id in [
            mux_command_ids::RESIZE_UP,
            mux_command_ids::RESIZE_DOWN,
            mux_command_ids::RESIZE_LEFT,
            mux_command_ids::RESIZE_RIGHT,
        ] {
            assert!(ids.iter().any(|candidate| candidate == id), "{id}");
        }
    }

    #[test]
    fn palette_lists_mux_focus_commands() {
        let ids = palette_entries()
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect::<Vec<_>>();

        for id in [
            mux_command_ids::NEXT_PANE,
            mux_command_ids::LAST_PANE,
            mux_command_ids::SWAP_PANE_NEXT,
            mux_command_ids::ZOOM_PANE,
            mux_command_ids::FOCUS_UP,
            mux_command_ids::FOCUS_DOWN,
            mux_command_ids::FOCUS_LEFT,
            mux_command_ids::FOCUS_RIGHT,
        ] {
            assert!(ids.iter().any(|candidate| candidate == id), "{id}");
        }
    }

    #[test]
    fn palette_lists_mux_select_window_commands() {
        let ids = palette_entries()
            .into_iter()
            .map(|entry| entry.id.to_string())
            .collect::<Vec<_>>();

        for id in mux_command_ids::SELECT_WINDOW {
            assert!(ids.iter().any(|candidate| candidate == id), "{id}");
        }
    }

    #[test]
    fn mux_swap_command_uses_the_headless_session_path() {
        let mut model = model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);

        model.run_command(mux_command_ids::SWAP_PANE_NEXT);

        assert_eq!(model.mux_session.active_window().active.get(), 2);
        assert_eq!(
            model
                .mux_session
                .active_window()
                .layout
                .leaves()
                .into_iter()
                .map(cockpit_mux::PaneId::get)
                .collect::<Vec<_>>(),
            vec![2, 1, 0]
        );
        assert!(model.status.contains("swapped pane-2"));
    }

    #[test]
    fn mux_zoom_command_projects_only_the_active_terminal_pane() {
        let mut model = primed_model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);
        model.run_command(mux_command_ids::SPLIT_VERTICAL);

        model.run_command(mux_command_ids::ZOOM_PANE);

        let terminal = model
            .last_layout
            .as_ref()
            .and_then(|layout| layout.terminal)
            .expect("terminal pane visible");
        let rects = model.mux_pane_rects_for_terminal(terminal);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].pane, model.mux_session.active_window().active);
        assert_eq!(model.mux_session.active_window().layout.leaves().len(), 3);
        assert!(model.status.contains("zoomed pane-2"));

        model.run_command(mux_command_ids::ZOOM_PANE);
        assert_eq!(model.mux_pane_rects_for_terminal(terminal).len(), 3);
        assert_eq!(model.mux_session.active_window().zoomed, None);
    }

    #[test]
    fn chord_to_terminal_bytes_encodes_keys_and_control_codes() {
        assert_eq!(chord_to_terminal_bytes(&chord("a")), Some(vec![b'a']));
        assert_eq!(chord_to_terminal_bytes(&chord("Enter")), Some(vec![b'\r']));
        assert_eq!(chord_to_terminal_bytes(&chord("Escape")), Some(vec![0x1b]));
        assert_eq!(
            chord_to_terminal_bytes(&chord("Backspace")),
            Some(vec![0x7f])
        );
        assert_eq!(
            chord_to_terminal_bytes(&chord("ArrowUp")),
            Some(b"\x1b[A".to_vec())
        );
        // Ctrl+C is the interrupt control byte.
        assert_eq!(chord_to_terminal_bytes(&chord("Ctrl+c")), Some(vec![0x03]));
        // Alt combos have no plain encoding.
        assert_eq!(chord_to_terminal_bytes(&chord("Alt+x")), None);
    }

    #[test]
    fn ctrl_p_opens_the_fuzzy_finder_indexed_over_the_project() {
        let mut model = model();
        assert!(model.finder.is_none());
        model.dispatch(chord("Ctrl+p"));

        let finder = model.finder.as_ref().expect("finder open");
        // The file-tree fixture has four indexable files (ignores filtered).
        assert_eq!(finder.items().len(), 4);
    }

    #[test]
    fn fuzzy_finder_filters_then_opens_the_selected_file() {
        let mut model = model();
        model.dispatch(chord("Ctrl+p"));
        type_keys(&mut model, "lib");
        model.dispatch(chord("Enter"));

        assert!(model.finder.is_none());
        assert_eq!(model.document.as_ref().unwrap().name, "lib.rs");
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn escape_closes_the_fuzzy_finder_without_opening_a_file() {
        let mut model = model();
        model.dispatch(chord("Ctrl+p"));
        model.dispatch(chord("Escape"));

        assert!(model.finder.is_none());
        assert!(model.document.is_none());
    }

    #[test]
    fn palette_mise_run_task_opens_the_task_picker() {
        let mut model = mise_model();
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "mise");
        model.dispatch(chord("Enter")); // activate "Mise: Run Task"

        let palette = model.palette.as_ref().expect("task picker open");
        assert_eq!(model.palette_mode, PaletteMode::MiseTasks);
        let ids: Vec<&str> = palette.entries().iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"lint"), "tasks: {ids:?}");
        assert!(ids.contains(&"test"), "tasks: {ids:?}");
    }

    #[test]
    fn running_a_mise_task_without_a_terminal_is_a_safe_no_op() {
        let mut model = mise_model();
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "mise");
        model.dispatch(chord("Enter")); // open the task picker
        model.dispatch(chord("Enter")); // run the highlighted task

        assert!(model.palette.is_none());
        // No redraw handle in tests → no terminal spawns; status reflects it.
        assert!(
            model.status.contains("terminal unavailable"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn mise_task_picker_is_empty_without_mise_tasks() {
        let mut model = model(); // file-tree fixture has no mise.toml
        model.dispatch(chord("Ctrl+Shift+p"));
        type_keys(&mut model, "mise");
        model.dispatch(chord("Enter"));

        // Nothing to pick: the picker does not open.
        assert!(model.palette.is_none());
        assert!(model.status.contains("No mise tasks"));
    }

    #[test]
    fn apply_cache_restores_pane_widths_and_the_active_file() {
        let mut model = model();
        let cache = ProjectCache {
            active_file: Some(fixture_path("file-tree").join("README.md")),
            left_width: Some(320),
            right_width: Some(400),
            ..ProjectCache::default()
        };
        model.apply_cache(cache);

        assert_eq!(model.layout.preferences().left_width, 320);
        assert_eq!(model.layout.preferences().right_width, 400);
        assert_eq!(model.document.as_ref().unwrap().name, "README.md");
    }

    #[test]
    fn apply_cache_ignores_a_missing_active_file() {
        let mut model = model();
        let cache = ProjectCache {
            active_file: Some(fixture_path("file-tree").join("does-not-exist.rs")),
            ..ProjectCache::default()
        };
        model.apply_cache(cache);
        assert!(model.document.is_none());
    }

    #[test]
    fn build_cache_snapshots_the_open_document_and_widths() {
        let (model, _dir) = open_temp_doc("hello");
        let cache = model.build_cache();

        assert!(cache.active_file.is_some());
        assert_eq!(cache.open_files.len(), 1);
        assert_eq!(cache.left_width, Some(260));
        assert_eq!(cache.right_width, Some(480));
    }

    // ---- M4.4 — Format on save -------------------------------------------

    #[test]
    fn apply_user_config_honours_format_on_save_and_layout_widths() {
        let mut model = model();
        assert!(!model.format_on_save);
        let toml = r#"
[ui]
left_width = 300
right_width = 360

[editor]
format_on_save = true
"#;
        let config = cockpit_config::Config::from_toml(toml).expect("config parses");
        model.apply_user_config(&config);

        assert!(model.format_on_save);
        assert_eq!(model.layout.preferences().left_width, 300);
        assert_eq!(model.layout.preferences().right_width, 360);
    }

    #[test]
    fn quarto_render_without_a_document_reports_clearly() {
        let mut model = model();
        model.run_command(QUARTO_RENDER);
        assert!(
            model.status.contains("No document"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn quarto_render_refuses_non_qmd_documents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.sql");
        std::fs::write(&path, "SELECT 1;\n").expect("seed file");
        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(path);

        model.run_command(QUARTO_RENDER);
        assert!(model.status.contains(".qmd"), "status: {}", model.status);
    }

    #[test]
    fn opening_a_jupytext_sql_file_populates_the_notebook_view_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("queries.sql");
        std::fs::write(&path, "-- %% cell\nSELECT 1;\n-- %% cell\nSELECT 2;\n").expect("seed file");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(path.clone());

        let notebook = model.notebook.as_ref().expect("notebook populated");
        assert_eq!(notebook.cells.len(), 2);
        assert_eq!(notebook.cells[0].source, "SELECT 1;");
        assert_eq!(notebook.cells[1].source, "SELECT 2;");
        assert!(model.status.contains("notebook"));
    }

    #[test]
    fn opening_a_plain_sql_file_leaves_notebook_view_model_unset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plain.sql");
        std::fs::write(&path, "SELECT 1;\n").expect("seed file");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(path);

        assert!(model.notebook.is_none(), "no notebook for plain SQL");
    }

    #[test]
    fn notebook_navigation_commands_walk_the_cursor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("queries.sql");
        std::fs::write(&path, "-- %% cell\nSELECT 1;\n-- %% cell\nSELECT 2;\n").expect("seed file");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(path);

        assert_eq!(model.notebook.as_ref().unwrap().active, 0);
        model.run_command(NOTEBOOK_NEXT_CELL);
        assert_eq!(model.notebook.as_ref().unwrap().active, 1);
        model.run_command(NOTEBOOK_PREVIOUS_CELL);
        assert_eq!(model.notebook.as_ref().unwrap().active, 0);
        model.run_command(NOTEBOOK_INSERT_CELL_BELOW);
        assert_eq!(model.notebook.as_ref().unwrap().cells.len(), 3);
    }

    #[test]
    fn show_dag_with_no_models_directory_reports_no_project() {
        let mut model = model();
        // The `file-tree` fixture has no `models/` dir.
        model.run_command(MODELS_SHOW_DAG);
        assert!(
            model.status.contains("No analytics project"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn show_dag_with_a_models_dir_reports_a_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("models")).expect("mkdir models");
        std::fs::write(
            dir.path().join("models").join("stg_orders.sql"),
            "SELECT 1 AS id",
        )
        .expect("seed model");
        std::fs::write(
            dir.path().join("models").join("fct_orders.sql"),
            "SELECT * FROM {{ ref('stg_orders') }}",
        )
        .expect("seed model");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");

        model.run_command(MODELS_SHOW_DAG);
        // Summary mentions both models in topological order.
        assert!(
            model.status.contains("stg_orders"),
            "status: {}",
            model.status
        );
        assert!(
            model.status.contains("fct_orders"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn editor_toggle_format_on_save_flips_the_session_preference() {
        let mut model = model();
        assert!(!model.format_on_save);

        model.run_command(EDITOR_TOGGLE_FORMAT_ON_SAVE);
        assert!(model.format_on_save);
        assert!(model.status.contains("ON"));

        model.run_command(EDITOR_TOGGLE_FORMAT_ON_SAVE);
        assert!(!model.format_on_save);
        assert!(model.status.contains("OFF"));
    }

    #[test]
    fn format_without_a_document_surfaces_a_clear_status() {
        let mut model = model();
        assert!(model.document.is_none());

        model.run_command(EDITOR_FORMAT);
        assert!(model.confirm.is_none());
        assert!(
            model.status.contains("No document to format"),
            "status: {}",
            model.status
        );
    }

    #[test]
    fn format_with_no_detectable_tool_falls_back_to_lsp_only_status() {
        // mise-basic has neither a `format` task nor a rustfmt entry; the
        // fixture has no .rs files so language detection drops out and the
        // request never reaches the LSP request path.
        let mut model = mise_model();
        let path = fixture_path("mise-basic").join("README.md");
        std::fs::write(&path, "# hello\n").expect("seed file");
        model.open_document(path.clone());
        assert!(model.document.is_some());

        model.run_command(EDITOR_FORMAT);
        // No formatter & no LSP server running for markdown → user-visible
        // explanation, not a panic.
        assert!(
            !model.status.is_empty(),
            "format should always set a status line"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn detected_formatter_in_tools_opens_the_add_format_task_prompt() {
        // Seed a tempdir with a `mise.toml` listing rustfmt as a tool so the
        // planner picks the "suggest mise task" branch deterministically.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("mise.toml"),
            "[tools]\nrustfmt = \"latest\"\n",
        )
        .expect("seed mise.toml");
        std::fs::write(dir.path().join("main.rs"), "fn main(){}\n").expect("seed source");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(dir.path().join("main.rs"));
        assert!(model.document.is_some());

        model.run_command(EDITOR_FORMAT);
        let prompt = model.confirm.as_ref().expect("confirm prompt is open");
        assert!(
            prompt.title().contains("rustfmt"),
            "title: {}",
            prompt.title()
        );
        assert!(prompt.body().contains("[tasks.format]"));
        assert!(!prompt.selection(), "default selection must be safe (No)");

        // Cancelling via `n` clears the prompt without touching mise.toml.
        model.dispatch(chord("n"));
        assert!(model.confirm.is_none());
        let mise_toml = std::fs::read_to_string(dir.path().join("mise.toml")).expect("read back");
        assert!(
            !mise_toml.contains("[tasks.format]"),
            "no task should be written on cancel — got:\n{mise_toml}"
        );
    }

    #[test]
    fn confirming_the_prompt_appends_format_task_to_mise_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("mise.toml"),
            "[tools]\nrustfmt = \"latest\"\n",
        )
        .expect("seed mise.toml");
        std::fs::write(dir.path().join("main.rs"), "fn main(){}\n").expect("seed source");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(dir.path().join("main.rs"));

        model.run_command(EDITOR_FORMAT);
        assert!(model.confirm.is_some());

        // Highlight Yes (Tab) then confirm with Enter — exercises the modal
        // contract that Enter is a no-op until the user opts into Yes.
        model.dispatch(chord("Tab"));
        assert!(model.confirm.as_ref().unwrap().selection());
        model.dispatch(chord("Enter"));

        // After accepting the prompt, the file has the snippet appended …
        let mise_toml = std::fs::read_to_string(dir.path().join("mise.toml")).expect("read back");
        assert!(
            mise_toml.contains("[tasks.format]"),
            "expected [tasks.format] in:\n{mise_toml}"
        );
        assert!(
            mise_toml.contains("rustfmt"),
            "expected suggested run line in:\n{mise_toml}"
        );
        // … and the live MiseProject is reparsed so a follow-up format would
        // pick the new task without restarting cockpit.
        assert!(
            model
                .detection
                .mise
                .tasks
                .iter()
                .any(|task| task.name == "format"),
            "detection should be refreshed after write"
        );
    }

    #[test]
    fn confirm_prompt_default_enter_cancels_without_writing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("mise.toml"),
            "[tools]\nrustfmt = \"latest\"\n",
        )
        .expect("seed mise.toml");
        std::fs::write(dir.path().join("main.rs"), "fn x(){}\n").expect("seed source");

        let detection = detect_project(dir.path()).expect("detect");
        let tree = FileTree::load(dir.path()).expect("tree");
        let mut model = AppModel::new(detection, tree).expect("model");
        model.open_document(dir.path().join("main.rs"));

        model.run_command(EDITOR_FORMAT);
        assert!(model.confirm.is_some());

        // Plain Enter with No still highlighted (default) must NOT write —
        // AGENTS.md rule #6.
        model.dispatch(chord("Enter"));
        assert!(model.confirm.is_none());
        let mise_toml = std::fs::read_to_string(dir.path().join("mise.toml")).expect("read back");
        assert!(
            !mise_toml.contains("[tasks.format]"),
            "Enter on No must cancel — got:\n{mise_toml}"
        );
    }

    // ---- M4.10 — Hermetic format flow over the injected env seam ---------

    /// `Arc`-shared `FileSystem` so the test and the [`AppModel`] both
    /// observe the same in-memory state.
    #[derive(Clone)]
    struct SharedFs(std::sync::Arc<cockpit_project::FakeFileSystem>);

    impl FileSystem for SharedFs {
        fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            self.0.read_to_string(path)
        }
        fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
            self.0.write(path, contents)
        }
        fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
            self.0.create_dir_all(path)
        }
        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }
    }

    /// `Arc`-shared `ProcessRunner` so the test can script responses and
    /// inspect the spawn log after the model has run.
    #[derive(Clone)]
    struct SharedProcess(std::sync::Arc<cockpit_project::FakeProcessRunner>);

    impl ProcessRunner for SharedProcess {
        fn run(&self, spec: &ProcessSpec) -> std::io::Result<cockpit_project::ProcessOutput> {
            self.0.run(spec)
        }
    }

    #[test]
    fn format_via_mise_task_runs_through_the_injected_process_runner() {
        use cockpit_project::ProcessOutput;

        let root = PathBuf::from("/proj");
        let main_rs = root.join("main.rs");

        // Seed an in-memory project: a mise.toml with a `format` task, and
        // the open document the formatter should rewrite. No real I/O.
        let fs = SharedFs(std::sync::Arc::new(cockpit_project::FakeFileSystem::new()));
        fs.0.insert_dir(&root);
        fs.0.insert_file(
            root.join("mise.toml"),
            "[tasks.format]\nrun = \"rustfmt $1\"\n",
        );
        fs.0.insert_file(&main_rs, "fn  main(){}\n");

        let process = SharedProcess(std::sync::Arc::new(
            cockpit_project::FakeProcessRunner::new(),
        ));
        // mise --version probe runs whenever we ask for project detection;
        // the format spawn arrives once the model dispatches `editor.format`.
        process.0.expect(
            "mise",
            ["--version"],
            ProcessOutput {
                success: true,
                stdout: b"mise 2026.0.0\n".to_vec(),
                stderr: Vec::new(),
            },
        );
        process.0.expect::<&str, _, std::ffi::OsString>(
            "mise",
            [
                std::ffi::OsString::from("run"),
                std::ffi::OsString::from("format"),
                std::ffi::OsString::from("--"),
                main_rs.clone().into_os_string(),
            ],
            ProcessOutput {
                success: true,
                stdout: Vec::new(),
                stderr: Vec::new(),
            },
        );

        // Detection is a pure function over the seeded fake project.
        let mise = cockpit_project::detect_mise_project_with(
            &root,
            &fs as &dyn FileSystem,
            &process as &dyn ProcessRunner,
        )
        .expect("hermetic mise detection");
        assert!(mise.tasks.iter().any(|t| t.name == "format"));

        let detection = ProjectDetection {
            root_path: root.clone(),
            display_name: "proj".to_string(),
            signals: Vec::new(),
            strongest_signal: None,
            mise,
        };
        // The file tree itself is not under test here; load it from a
        // bundled fixture so the model has something to populate the
        // browser pane with. The browser is irrelevant to the format flow.
        let tree =
            FileTree::load(fixture_path("file-tree")).expect("load file-tree fixture for shell");
        let mut model = AppModel::with_env(
            detection,
            tree,
            Box::new(fs.clone()),
            Box::new(process.clone()),
        )
        .expect("hermetic model");

        // Open the in-memory file through the model's own document path.
        model.open_document(main_rs.clone());
        assert!(model.document.is_some(), "document should open via fake fs");

        // Simulate the formatter rewriting the file on disk so the reload
        // observes a change.
        fs.0.insert_file(&main_rs, "fn main() {}\n");

        model.run_command(EDITOR_FORMAT);
        assert!(
            model.status.contains("Formatted via"),
            "status: {}",
            model.status
        );
        // The injected process runner saw both scripted spawns:
        // `mise --version` during detection above, then `mise run format`
        // — proving the model never touches `std::process::Command` and
        // every spawn is observable from the test (M4.10 payoff).
        let log = process.0.spawns();
        assert_eq!(log.len(), 2, "spawn count, log: {log:?}");
        assert_eq!(log[0].args, vec!["--version"]);
        assert_eq!(log[1].args[0], "run");
        assert_eq!(log[1].args[1], "format");
        // And the buffer reloaded from the fake fs, not from real disk.
        assert_eq!(
            model.document.as_ref().unwrap().editor.text(),
            "fn main() {}\n"
        );
    }

    // ---- M4.7 — Mouse input ----------------------------------------------

    /// Compute a layout for the model so the mouse handlers have a real
    /// rectangle tree to hit-test against. Returns the model + the layout
    /// for assertions.
    fn primed_model() -> AppModel {
        let mut model = model();
        let mut painter = Painter::new();
        model.paint(
            &mut painter,
            Viewport {
                width: 1280,
                height: 800,
                scale: 1.0,
            },
        );
        model
    }

    #[test]
    fn clicking_inside_the_files_pane_focuses_files() {
        let mut model = primed_model();
        // Start with the editor focused so the click is observable.
        model.layout.focus(PaneId::Editor);
        assert_eq!(model.layout.focused(), PaneId::Editor);

        // The files pane is the leftmost — a click at (10, 200) is safely inside.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(10.0, 200.0));
        assert_eq!(model.layout.focused(), PaneId::Files);
    }

    #[test]
    fn clicking_inside_the_editor_pane_focuses_editor() {
        let mut model = primed_model();
        model.layout.focus(PaneId::Files);
        // Default layout: files=260, editor=460, terminal=480 over 1280px.
        // Editor center is around x=490.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(490.0, 300.0));
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn clicking_inside_the_terminal_pane_focuses_terminal() {
        let mut model = primed_model();
        // Terminal sits at x >= 800 in the default 1280-wide layout.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(1000.0, 300.0));
        assert_eq!(model.layout.focused(), PaneId::Terminal);
    }

    #[test]
    fn clicking_a_mux_terminal_pane_focuses_that_pane() {
        let mut model = primed_model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);

        // Default 1280-wide layout: terminal content starts at x=800,
        // y=TOP_BAR_H+HEADER_H. The right half belongs to pane-1.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(1100.0, 120.0));
        assert_eq!(model.mux_session.active_window().active.get(), 1);

        model.on_pointer_down(MouseButton::Left, PointerPosition::new(850.0, 120.0));
        assert_eq!(model.mux_session.active_window().active.get(), 0);
        assert_eq!(model.layout.focused(), PaneId::Terminal);
    }

    #[test]
    fn right_click_does_not_change_focus() {
        let mut model = primed_model();
        model.layout.focus(PaneId::Editor);
        // Right-click inside the files pane must NOT steal focus —
        // right-button is reserved for future context menus.
        model.on_pointer_down(MouseButton::Right, PointerPosition::new(10.0, 200.0));
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn dragging_the_left_pane_border_resizes_the_files_pane() {
        let mut model = primed_model();
        let initial = model.layout.preferences().left_width;
        // Press on the left border (default files width = 260) then drag right.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(260.0, 200.0));
        model.on_pointer_move(PointerPosition::new(320.0, 200.0));
        model.on_pointer_up(MouseButton::Left, PointerPosition::new(320.0, 200.0));
        assert!(
            model.layout.preferences().left_width > initial,
            "left width {} should exceed initial {initial}",
            model.layout.preferences().left_width
        );
    }

    #[test]
    fn dragging_the_right_pane_border_resizes_the_terminal_pane() {
        let mut model = primed_model();
        let initial = model.layout.preferences().right_width;
        // Right border lives at x = 800 in the default layout.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(800.0, 200.0));
        model.on_pointer_move(PointerPosition::new(700.0, 200.0));
        model.on_pointer_up(MouseButton::Left, PointerPosition::new(700.0, 200.0));
        assert!(
            model.layout.preferences().right_width > initial,
            "right width {} should exceed initial {initial}",
            model.layout.preferences().right_width
        );
    }

    #[test]
    fn dragging_a_mux_terminal_border_resizes_the_split() {
        let mut model = primed_model();
        model.run_command(mux_command_ids::SPLIT_HORIZONTAL);
        let left_pane = model.mux_session.active_window().layout.leaves()[0];
        model
            .mux_session
            .select_pane(left_pane)
            .expect("left pane can be focused before dragging divider");

        // Default 1280-wide layout: terminal content spans roughly
        // x=800..1280, so the 50/50 mux split border is near x=1040.
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(1040.0, 120.0));
        model.on_pointer_move(PointerPosition::new(1088.0, 120.0));
        model.on_pointer_up(MouseButton::Left, PointerPosition::new(1088.0, 120.0));

        match &model.mux_session.active_window().layout {
            cockpit_mux::LayoutNode::Split { ratio, .. } => {
                assert!(
                    *ratio > 0.55,
                    "dragging right should move the split ratio right even when the left pane was active, got {ratio}"
                );
            }
            other => panic!("expected split layout, got {other:?}"),
        }
    }

    #[test]
    fn dragging_a_vertical_mux_terminal_border_resizes_the_split() {
        let mut model = primed_model();
        model.run_command(mux_command_ids::SPLIT_VERTICAL);

        let terminal = model
            .last_layout
            .as_ref()
            .and_then(|layout| layout.terminal)
            .expect("terminal pane visible");
        let rects = model.mux_pane_rects_for_terminal(terminal);
        let top = rects
            .iter()
            .find(|pane| pane.pane.get() == 0)
            .expect("top pane projected");
        let border_y = (top.rect.y + top.rect.height) as f32;
        let x = top.rect.x as f32 + top.rect.width as f32 / 2.0;

        model.on_pointer_down(MouseButton::Left, PointerPosition::new(x, border_y));
        model.on_pointer_move(PointerPosition::new(x, border_y + 72.0));
        model.on_pointer_up(MouseButton::Left, PointerPosition::new(x, border_y + 72.0));

        match &model.mux_session.active_window().layout {
            cockpit_mux::LayoutNode::Split { ratio, .. } => {
                assert!(
                    *ratio > 0.55,
                    "dragging down should move the split ratio down, got {ratio}"
                );
            }
            other => panic!("expected split layout, got {other:?}"),
        }
    }

    #[test]
    fn scroll_in_the_editor_pane_updates_the_scroll_offset() {
        let mut model = primed_model();
        assert_eq!(model.editor_scroll, 0.0);
        // Scroll up — content scrolls up so editor_scroll grows.
        model.on_scroll(PointerPosition::new(490.0, 300.0), 0.0, -40.0);
        assert!(model.editor_scroll > 0.0, "scroll: {}", model.editor_scroll);
        // Scrolling back down clamps at zero, not negative.
        model.on_scroll(PointerPosition::new(490.0, 300.0), 0.0, 200.0);
        assert_eq!(model.editor_scroll, 0.0);
    }

    #[test]
    fn clicking_a_file_row_selects_and_opens_it() {
        let mut model = primed_model();
        assert!(model.document.is_none(), "no document open at start");

        // Row 2 in the file-tree fixture is README.md (rows: src, tests,
        // README.md). Click lands at pane_top + ROW_H * 2 + a few pixels.
        let row_y = TOP_BAR_H + HEADER_H + PAD * 0.5 + ROW_H * 2.0 + 4.0;
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(10.0, row_y));

        // Opening a file leaves focus on the editor (open_document's contract).
        assert_eq!(model.layout.focused(), PaneId::Editor);
        let doc = model.document.as_ref().expect("a file should be open");
        assert_eq!(doc.name, "README.md");
    }

    #[test]
    fn clicking_a_directory_row_expands_it_and_keeps_files_focused() {
        let mut model = primed_model();

        // Row 0 is `src` — a directory in the file-tree fixture.
        let row_y = TOP_BAR_H + HEADER_H + PAD * 0.5 + 4.0;
        model.on_pointer_down(MouseButton::Left, PointerPosition::new(10.0, row_y));
        // Directory toggles don't open a document and don't move focus
        // away from the files pane.
        assert!(model.document.is_none());
        assert_eq!(model.layout.focused(), PaneId::Files);
        assert!(
            model.browser.rows().iter().any(|row| row.name == "lib.rs"),
            "src should be expanded after the click"
        );
    }
}

/// UI smoke tests (spec §18.8 / M3.6).
///
/// Asserts on the headless `cockpit-ui` view-model tree rather than pixels —
/// the spec is explicit that pixel-level coverage is out of scope. Gated
/// behind the `ui-smoke` Cargo feature so they stay out of the default test
/// run; CI has a dedicated `ui-smoke` leg that enables them.
#[cfg(all(test, feature = "ui-smoke"))]
mod ui_smoke {
    use super::*;
    use cockpit_project::{detect_project, recent_projects_path};
    use cockpit_testkit::fixture_path;
    use cockpit_ui::{Launcher, LauncherAction, LauncherSelection, RecentProject};

    /// Build an `AppModel` over the `rust-basic` fixture — the canonical
    /// smoke-test project (it has a Cargo.toml and `src/main.rs`).
    fn smoke_model() -> AppModel {
        let path = fixture_path("rust-basic");
        let detection = detect_project(&path).expect("detect rust-basic fixture");
        let tree = FileTree::load(&path).expect("load rust-basic fixture");
        AppModel::new(detection, tree).expect("build AppModel")
    }

    #[test]
    fn smoke_app_starts() {
        let model = smoke_model();
        assert!(!model.exit, "app must not be exiting at startup");
        assert!(
            !model.status.is_empty(),
            "the app should announce itself in the status line"
        );
    }

    #[test]
    fn smoke_project_launcher_renders() {
        let recents = vec![
            RecentProject::new("alpha", "/code/alpha"),
            RecentProject::new("bravo", "/code/bravo"),
        ];
        let launcher = Launcher::new(recents);
        assert_eq!(launcher.recents().len(), 2);
        assert_eq!(launcher.actions(), LauncherAction::ALL);
        // With recents present, the cursor lands on the first recent project.
        assert_eq!(launcher.selection(), LauncherSelection::Recent(0));

        // The launcher must also render cleanly with no recent projects.
        let empty = Launcher::new(Vec::new());
        assert!(empty.recents().is_empty());
        assert_eq!(
            empty.selection(),
            LauncherSelection::Action(LauncherAction::OpenFolder),
        );

        // And the recent-projects cache file path must be resolvable so the
        // binary can persist the launcher state at all.
        recent_projects_path().expect("recent-projects path resolves");
    }

    #[test]
    fn smoke_project_opens() {
        let model = smoke_model();
        // Detection must produce a project with a usable display name and at
        // least one signal — that is the "opened project" view-model.
        assert!(!model.detection.display_name.is_empty());
        assert!(
            model.detection.detected(),
            "rust-basic fixture should detect at least one project signal",
        );
        // The file-browser view-model has visible rows for the project root.
        assert!(
            !model.browser.rows().is_empty(),
            "file browser should populate from the project tree",
        );
    }

    #[test]
    fn smoke_three_panes_render() {
        let model = smoke_model();
        let computed = model.layout.compute(1600, 900);
        assert!(
            computed.files.is_some(),
            "files pane must render in the default layout",
        );
        assert!(
            computed.editor.width > 0,
            "editor pane must have non-zero width",
        );
        assert!(
            computed.terminal.is_some(),
            "terminal pane must render in the default layout",
        );
    }

    #[test]
    fn smoke_file_can_be_opened() {
        let mut model = smoke_model();
        assert!(model.document.is_none(), "no file is open at startup");
        model.open_path_reference("src/main.rs");
        let doc = model.document.as_ref().expect("file opens");
        assert_eq!(doc.name, "main.rs");
        assert_eq!(model.layout.focused(), PaneId::Editor);
    }

    #[test]
    fn smoke_terminal_pane_can_be_created() {
        let mut model = smoke_model();
        // Focusing the terminal pane is the user-visible action that "creates"
        // it from the layout view-model's perspective. The actual PTY spawn is
        // covered separately by the integration leg.
        model.run_command(command_ids::FOCUS_TERMINAL);
        assert_eq!(model.layout.focused(), PaneId::Terminal);
        let computed = model.layout.compute(1600, 900);
        assert!(computed.terminal.is_some(), "terminal pane must be visible");
    }

    #[test]
    fn smoke_basic_keybindings_work() {
        let mut model = smoke_model();
        // The default global key map (cockpit-config::GlobalKeys::default())
        // routes Ctrl+h → focus files, Ctrl+l → focus terminal, Ctrl+j → editor.
        model.dispatch("Ctrl+h".parse().expect("valid chord"));
        assert_eq!(model.layout.focused(), PaneId::Files);
        model.dispatch("Ctrl+l".parse().expect("valid chord"));
        assert_eq!(model.layout.focused(), PaneId::Terminal);
        model.dispatch("Ctrl+j".parse().expect("valid chord"));
        assert_eq!(model.layout.focused(), PaneId::Editor);
        // Every chord that traversed dispatch should have been recorded.
        assert_eq!(model.key_log.len(), 3);
    }

    #[test]
    fn smoke_app_exits_cleanly() {
        let mut model = smoke_model();
        assert!(!model.wants_exit());
        model.run_command(APP_QUIT);
        assert!(model.wants_exit(), "App: Quit must set the wants_exit flag",);
    }

    /// M6.2: shell starts in the hydrating state, asks the harness for
    /// continuous redraws, and walks through every phase via `tick`
    /// until it transitions to `Live`.
    #[test]
    fn app_shell_hydrates_through_tick_to_a_live_model() {
        let mut shell = AppShell::hydrating(fixture_path("rust-basic"));
        assert!(shell.is_hydrating());
        assert!(shell.wants_continuous_redraw());
        assert!(!shell.is_live());

        // Bounded loop: every phase advances on one tick, and the
        // terminal `Ready` consumes one more.
        for _ in 0..cockpit_ui::HydrationPhase::ALL.len() + 1 {
            if shell.is_live() {
                break;
            }
            shell.tick();
        }
        assert!(
            shell.is_live(),
            "shell should have transitioned to live after hydration"
        );
        assert!(!shell.wants_continuous_redraw());
        assert_eq!(
            shell.model().expect("live model").project_name(),
            "rust-basic"
        );
    }

    /// M6.2: a missing-path hydration parks the shell in the failed
    /// state without panicking, and `wants_continuous_redraw` flips off
    /// so the harness stops spinning frames.
    #[test]
    fn app_shell_lands_in_failed_state_on_bad_path() {
        let mut shell = AppShell::hydrating(std::path::PathBuf::from(
            "/this/path/should/not/exist/anywhere",
        ));
        for _ in 0..cockpit_ui::HydrationPhase::ALL.len() + 1 {
            if shell.is_failed() {
                break;
            }
            shell.tick();
        }
        assert!(shell.is_failed(), "shell should have failed hydration");
        assert!(!shell.wants_continuous_redraw());
        assert!(shell.model().is_none());
    }

    /// M7.1: the shell starts on the launcher, transitions to hydrating
    /// when the user picks a recent project, and lands on a live model
    /// — all without restarting the harness.
    #[test]
    fn app_shell_launcher_hands_off_to_hydration_in_place() {
        let recent =
            cockpit_ui::launcher::RecentProject::new("rust-basic", fixture_path("rust-basic"));
        let launcher = crate::launcher::LauncherModel::new(vec![recent]);
        let mut shell = AppShell::launcher(launcher);

        assert!(shell.is_launcher());
        assert!(
            shell.wants_continuous_redraw(),
            "launcher needs continuous redraws so tick() can spot the result"
        );
        assert!(!shell.wants_exit());

        // Selection lands on the only recent by default; Enter activates.
        shell.on_key("Enter".parse().expect("valid chord"));
        // One tick consumes the result and transitions to hydrating —
        // no second event loop, no `run_app` re-entry (M7.1 hard rule).
        shell.tick();
        assert!(
            shell.is_hydrating(),
            "Enter on a recent project should hand off to hydration"
        );

        // Drive hydration through to a live model.
        for _ in 0..cockpit_ui::HydrationPhase::ALL.len() + 1 {
            if shell.is_live() {
                break;
            }
            shell.tick();
        }
        assert!(
            shell.is_live(),
            "shell should have hydrated the picked project"
        );
        assert_eq!(
            shell.model().expect("live model").project_name(),
            "rust-basic"
        );
    }

    /// M7.1: pressing Escape in the launcher requests exit cleanly —
    /// the harness sees `wants_exit` and the event loop drops out
    /// without ever opening a project window.
    #[test]
    fn app_shell_launcher_escape_requests_exit() {
        let launcher = crate::launcher::LauncherModel::new(vec![]);
        let mut shell = AppShell::launcher(launcher);
        assert!(!shell.wants_exit());

        shell.on_key("Escape".parse().expect("valid chord"));
        shell.tick();
        assert!(
            shell.wants_exit(),
            "Escape in the launcher should request a clean exit"
        );
        assert!(
            shell.is_launcher(),
            "exit is requested from the launcher state itself; no transition needed"
        );
    }
}
