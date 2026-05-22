//! The application shell — Track D wire-up.
//!
//! [`AppModel`] is the headless, plain-data application state: the project,
//! the file-browser view-model, the workspace layout, the global input router,
//! and the open editor document. It turns key chords into state changes and
//! paints itself into a [`Painter`]. [`AppShell`] is the thin [`CockpitApp`]
//! adapter the windowing harness drives — all real logic lives in [`AppModel`],
//! so it stays testable without a window (AGENTS §2).

use std::path::PathBuf;

use cockpit_commands::{KeyChord, Modifiers};
use cockpit_config::GlobalKeys;
use cockpit_editor::vim::{Key as VimKey, Mode};
use cockpit_editor::{Editor, EditorSignal};
use cockpit_project::{
    FileNodeKind, FileTree, ProjectCache, ProjectDetection, project_cache_path, walk_project_files,
};
use cockpit_render::theme::Color;
use cockpit_render::{CockpitApp, Painter, Rect as RenderRect, RedrawHandle, Theme, Viewport};
use cockpit_terminal::live::{LiveTerminal, WakeFn};
use cockpit_terminal::pty::PtyDimensions;
use cockpit_terminal::session::TerminalStatus;
use cockpit_terminal::zellij::{LaunchPlan, PathBinaryLookup, ShellProfile, plan_launch};
use cockpit_ui::{
    ComputedLayout, FileBrowser, FileBrowserAction, FuzzyFinder, InputRouter, Palette,
    PaletteEntry, PaneId, Rect as UiRect, RoutedInput, WorkspaceLayout, command_ids,
};

/// Logical layout metrics. The painter scales these by the display factor.
const TOP_BAR_H: f32 = 30.0;
const HEADER_H: f32 = 24.0;
const ROW_H: f32 = 20.0;
const FONT: f32 = 13.0;
const PAD: f32 = 8.0;
const GUTTER_W: f32 = 52.0;
const INDENT_W: f32 = 14.0;
/// Monospace advance estimate, as a fraction of the font size.
const CHAR_W_RATIO: f32 = 0.6;

/// Command id for "quit the application" — handled directly by the shell.
const APP_QUIT: &str = "app.quit";
/// Command id for "pick and run a mise task".
const MISE_RUN_TASK: &str = "mise.run_task";

/// What activating a palette entry does — the palette is reused as both the
/// command palette and the mise-task picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    /// Entries are app commands dispatched through `run_command`.
    Commands,
    /// Entries are mise task names sent to the terminal.
    MiseTasks,
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

    /// Apply a resolved global command.
    fn run_command(&mut self, id: &str) {
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

    /// Spawn the terminal session on first use of the terminal pane.
    fn ensure_terminal(&mut self) {
        if self.terminal.is_some() {
            return;
        }
        let Some(redraw) = self.redraw.clone() else {
            self.status = "Terminal unavailable — no redraw handle.".to_string();
            return;
        };
        let plan = plan_launch(
            &self.detection.display_name,
            &PathBinaryLookup,
            ShellProfile::host_default(),
        );
        let (command, label) = match plan {
            LaunchPlan::Zellij(command) => (command, "zellij".to_string()),
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
                self.document = Some(OpenDocument {
                    editor: Editor::new(&content),
                    path,
                    name,
                });
                self.layout.focus(PaneId::Editor);
            }
            Err(err) => self.status = format!("Could not open {}: {err}", path.display()),
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

    /// Paint the whole window for one frame.
    pub fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
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
            let text_x = content.x + PAD + row.depth as f32 * INDENT_W;
            let color = if row.is_dir() {
                self.theme.text
            } else {
                self.theme.muted_text
            };
            canvas.text(
                text_x,
                row_y + 3.0,
                format!("{marker}{}", row.name),
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
            if !line.is_empty() {
                canvas.text(
                    content.x + GUTTER_W,
                    line_y,
                    (*line).to_string(),
                    self.theme.text,
                    FONT,
                );
            }
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
