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

use cockpit_commands::{KeyChord, Modifiers};
use cockpit_config::{GlobalKeys, ZellijLayout};
use cockpit_editor::vim::{Key as VimKey, Mode};
use cockpit_editor::{
    Editor, EditorSignal, HighlightKind, HighlightSpan, Language, nearest_test_name,
};
use cockpit_lsp::{
    Diagnostic, DiagnosticSeverity, LspClient, PublishDiagnosticsParams, RecvMessage, ServerConfig,
};
use cockpit_project::{
    FileNodeKind, FileTree, ProjectCache, ProjectDetection, mise_exec_command, project_cache_path,
    walk_project_files,
};
use cockpit_render::theme::Color;
use cockpit_render::{CockpitApp, Painter, Rect as RenderRect, RedrawHandle, Theme, Viewport};
use cockpit_terminal::bridge::{detect_paths_in_grid, paste_to_terminal, render_document_path};
use cockpit_terminal::live::{LiveTerminal, WakeFn};
use cockpit_terminal::path_detect::detect_paths;
use cockpit_terminal::pty::PtyDimensions;
use cockpit_terminal::session::TerminalStatus;
use cockpit_terminal::zellij::{LaunchPlan, PathBinaryLookup, ShellProfile, plan_launch};
use cockpit_ui::{
    ComputedLayout, FileBrowser, FileBrowserAction, FuzzyFinder, InputRouter, Palette,
    PaletteEntry, PaneId, Rect as UiRect, RoutedInput, WorkspaceLayout, command_ids,
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

/// Headless application state for the v0.1 cockpit shell.
pub struct AppModel {
    detection: ProjectDetection,
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
    terminal: Option<LiveTerminal>,
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
    exit: bool,
}

impl AppModel {
    /// Build the model for a detected project and its loaded file tree.
    ///
    /// This is pure — call [`restore_cached_state`](Self::restore_cached_state)
    /// afterwards to load persisted per-project state from disk.
    pub fn new(detection: ProjectDetection, tree: FileTree) -> Result<Self, String> {
        let router = InputRouter::from_global_keys(&GlobalKeys::default())
            .map_err(|err| format!("input router setup failed: {err:?}"))?;
        Ok(Self {
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
            terminal: None,
            redraw: None,
            cache_path: None,
            key_log: VecDeque::with_capacity(DEBUG_LOG_SIZE),
            command_log: VecDeque::with_capacity(DEBUG_LOG_SIZE),
            lsp_clients: HashMap::new(),
            lsp_initialized: HashSet::new(),
            diagnostics: HashMap::new(),
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

    /// Best-effort refresh of git status badges (spec §23 v0.3 / M3.4). Shells
    /// out to `git status --porcelain`; no-ops when `git` is missing or the
    /// project is not a git working tree.
    pub fn refresh_git_status(&mut self) {
        let statuses = cockpit_project::git_status(&self.detection.root_path);
        self.browser.set_git_statuses(statuses);
    }

    /// Restore persisted pane widths and reopen the last active file.
    fn apply_cache(&mut self, cache: ProjectCache) {
        let mut prefs = self.layout.preferences().clone();
        if let Some(width) = cache.left_width {
            prefs.left_width = u32::from(width);
        }
        if let Some(width) = cache.right_width {
            prefs.right_width = u32::from(width);
        }
        self.layout.set_preferences(prefs);

        if let Some(active) = cache.active_file
            && active.is_file()
        {
            self.open_document(active);
        }
    }

    /// Snapshot the per-project state worth persisting across sessions.
    fn build_cache(&self) -> ProjectCache {
        let active_file = self.document.as_ref().map(|doc| doc.path.clone());
        let prefs = self.layout.preferences();
        ProjectCache {
            open_files: active_file.clone().into_iter().collect(),
            active_file,
            left_width: Some(prefs.left_width as u16),
            right_width: Some(prefs.right_width as u16),
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
        if self.palette.is_some() {
            self.handle_palette_key(&chord);
            return;
        }
        if self.finder.is_some() {
            self.handle_finder_key(&chord);
            return;
        }
        let focused = self.layout.focused();
        match self.router.route(focused, chord) {
            RoutedInput::Command(id) => self.run_command(id.as_str()),
            RoutedInput::Unhandled(chord) | RoutedInput::TerminalPassthrough(chord) => {
                self.handle_local(focused, &chord)
            }
        }
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

    /// Send `mise run <task>` to the terminal session, starting it if needed.
    fn run_mise_task(&mut self, task: &str) {
        self.ensure_terminal();
        let command = format!("mise run {task}\r");
        match self.terminal.as_mut() {
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
        match self.terminal.as_mut() {
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
        match self.terminal.as_mut() {
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
        let term = match &self.terminal {
            Some(_) => "spawned",
            None => "none",
        };
        format!(
            "files={}px, terminal={}px, focused={:?}, terminal_proc={term}",
            prefs.left_width, prefs.right_width, focused,
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
        let Some(terminal) = self.terminal.as_ref() else {
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

    /// Spawn the terminal session on first use of the terminal pane.
    fn ensure_terminal(&mut self) {
        if self.terminal.is_some() {
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
                self.terminal = Some(terminal);
                self.status = format!("Terminal started ({label}).");
            }
            Err(err) => self.status = format!("Terminal failed to start: {err}"),
        }
    }

    /// Forward a chord to the PTY when the terminal pane is focused.
    fn handle_terminal_key(&mut self, chord: &KeyChord) {
        let Some(bytes) = chord_to_terminal_bytes(chord) else {
            return;
        };
        let Some(terminal) = self.terminal.as_mut() else {
            return;
        };
        if let Err(err) = terminal.send_input(&bytes) {
            self.status = format!("Terminal write failed: {err}");
        }
    }

    /// Resize the live terminal so its grid matches the terminal pane.
    fn sync_terminal_size(&mut self, rect: UiRect) {
        let Some(terminal) = self.terminal.as_mut() else {
            return;
        };
        let char_w = FONT * CHAR_W_RATIO;
        let inner_w = (rect.width as f32 - 2.0 * PAD).max(char_w);
        let inner_h = (rect.height as f32 - HEADER_H - PAD).max(ROW_H);
        let cols = (inner_w / char_w) as u16;
        let rows = (inner_h / ROW_H) as u16;
        let _ = terminal.resize(PtyDimensions::new(rows, cols));
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
        let Some(key) = chord_to_vim_key(chord) else {
            return;
        };
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

    /// Load a file into the editor pane.
    fn open_document(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.status = format!("Opened {name}");
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

    /// Write the open document to its file.
    fn save_document(&mut self) {
        let Some(doc) = self.document.as_mut() else {
            self.status = "No document to save.".to_string();
            return;
        };
        match std::fs::write(&doc.path, doc.editor.text()) {
            Ok(()) => {
                doc.editor.mark_saved();
                self.status = format!("Saved {}", doc.name);
            }
            Err(err) => self.status = format!("Save failed: {err}"),
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

    /// Paint the whole window for one frame.
    pub fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
        self.drain_lsp_messages();
        let scale = viewport.scale.max(0.5);
        let width = viewport.width as f32 / scale;
        let height = viewport.height as f32 / scale;

        let body_height = (height - TOP_BAR_H).max(0.0);
        let computed: ComputedLayout = self.layout.compute(width as u32, body_height as u32);

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
        let first = cursor_line.saturating_sub(visible.saturating_sub(1));

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
        let Some(terminal) = &self.terminal else {
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
            return;
        };

        let snapshot = terminal.snapshot();
        let grid = &snapshot.grid;
        let char_w = FONT * CHAR_W_RATIO;
        let top = content.y + PAD * 0.5;

        for row in 0..grid.height() {
            let Some(text) = grid.row_text(row) else {
                continue;
            };
            let trimmed = text.trim_end();
            if !trimmed.is_empty() {
                canvas.text(
                    content.x + PAD,
                    top + row as f32 * ROW_H + 3.0,
                    trimmed.to_string(),
                    self.theme.text,
                    FONT,
                );
            }
        }

        let cursor = grid.cursor();
        let cursor_x = content.x + PAD + cursor.col as f32 * char_w;
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
                content.x + PAD,
                content.y + content.h - ROW_H + 3.0,
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
fn lsp_launch_argv(config: &ServerConfig) -> Vec<String> {
    let inner: Vec<&str> = std::iter::once(config.command.as_str())
        .chain(config.args.iter().map(String::as_str))
        .collect();
    mise_exec_command(&inner)
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
        PaletteEntry::new(TEST_RUN_ALL, "Test: Run All"),
        PaletteEntry::new(TEST_RUN_CURRENT_FILE, "Test: Run Current File"),
        PaletteEntry::new(TEST_RUN_NEAREST, "Test: Run Nearest"),
        PaletteEntry::new(DEBUG_SHOW_KEY_EVENTS, "Debug: Show Key Events"),
        PaletteEntry::new(DEBUG_SHOW_COMMAND_LOG, "Debug: Show Command Log"),
        PaletteEntry::new(DEBUG_SHOW_PANE_TREE, "Debug: Show Pane Tree"),
        PaletteEntry::new(DEBUG_SHOW_PROJECT_STATE, "Debug: Show Project State"),
        PaletteEntry::new(DEBUG_RELOAD_CONFIG, "Debug: Reload Config"),
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

/// [`CockpitApp`] adapter: forwards harness callbacks to the [`AppModel`].
pub struct AppShell {
    model: AppModel,
}

impl AppShell {
    /// Wrap a model for the windowing harness.
    pub fn new(model: AppModel) -> Self {
        Self { model }
    }
}

impl CockpitApp for AppShell {
    fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
        self.model.paint(painter, viewport);
    }

    fn theme(&self) -> &Theme {
        &self.model.theme
    }

    fn on_key(&mut self, chord: KeyChord) {
        self.model.dispatch(chord);
    }

    fn set_redraw_handle(&mut self, handle: RedrawHandle) {
        self.model.set_redraw_handle(handle);
    }

    fn on_shutdown(&mut self) {
        self.model.on_shutdown();
    }

    fn wants_exit(&self) -> bool {
        self.model.wants_exit()
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
        assert!(model.terminal.is_none());
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
}
