//! Org-mode view-models (v0.12 M12.5).
//!
//! Pure-data view-models over [`cockpit_org`], reusable from both the main
//! cockpit (floating agenda pane, M12.7) and the jot popover (M12.6). Same
//! layering as the HTTP view ([`crate::http`]) and the notebook: the domain
//! crate owns parsing/agenda/capture; this module owns "what the panel shows
//! and where the cursor is". No GPU, no clock, no filesystem — `today` and
//! `now` are injected by the caller.
//!
//! Three view-models:
//! - [`OrgListView`] — the org overview: every file and its headings, with a
//!   cursor that lands on headings for jump-to.
//! - [`AgendaView`] — Today / Next-7-days / TODO-list, with mode cycling,
//!   tag/keyword filtering, and a cursor over the dated items.
//! - [`CaptureView`] — the capture flow: template picker → single-field editor
//!   with the `%?` cursor pre-positioned, committing an entry ready for
//!   [`cockpit_org::apply_capture`].

use std::path::{Path, PathBuf};

use cockpit_org::today as agenda_today;
use cockpit_org::{
    AgendaItem, AgendaKind, CaptureContext, CaptureTarget, CaptureTemplate, Expansion, Filter,
    NowStamp, OrgConfig, OrgRoot, date, next_7_days, todo_list,
};

// ---- OrgListView -------------------------------------------------------------

/// One row in the org overview: a file header or an (indented) heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgListRow {
    /// File this row belongs to.
    pub file: PathBuf,
    /// `true` for a file-header row (non-selectable), `false` for a heading.
    pub is_file_header: bool,
    /// Heading depth (1-based). `0` for file headers.
    pub level: usize,
    /// Display label (file name, or the heading title).
    pub label: String,
    /// TODO keyword, if the heading has one.
    pub todo_keyword: Option<String>,
    /// Tags on the heading.
    pub tags: Vec<String>,
    /// Headline line (0-based) for jump-to. `0` for file headers.
    pub line: usize,
}

/// The org overview view-model: a flat, navigable list of files and headings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrgListView {
    rows: Vec<OrgListRow>,
    /// Indices into `rows` that the cursor may land on (headings only).
    selectable: Vec<usize>,
    cursor: usize,
}

impl OrgListView {
    /// Build the overview from a root: each file's name followed by its
    /// headings in document order.
    pub fn build(root: &OrgRoot) -> Self {
        let mut rows = Vec::new();
        let mut selectable = Vec::new();
        for file in root.files.values() {
            rows.push(OrgListRow {
                file: file.path.clone(),
                is_file_header: true,
                level: 0,
                label: file_name(&file.path),
                todo_keyword: None,
                tags: Vec::new(),
                line: 0,
            });
            for heading in file.iter_headings() {
                selectable.push(rows.len());
                rows.push(OrgListRow {
                    file: file.path.clone(),
                    is_file_header: false,
                    level: heading.level,
                    label: heading.title.clone(),
                    todo_keyword: heading.todo_keyword.clone(),
                    tags: heading.tags.clone(),
                    line: heading.line_range.start,
                });
            }
        }
        OrgListView {
            rows,
            selectable,
            cursor: 0,
        }
    }

    /// All rows, for the painter.
    pub fn rows(&self) -> &[OrgListRow] {
        &self.rows
    }

    /// The currently selected row index within `rows`, if any heading exists.
    pub fn selected_index(&self) -> Option<usize> {
        self.selectable.get(self.cursor).copied()
    }

    /// The selected heading row, if any.
    pub fn selected(&self) -> Option<&OrgListRow> {
        self.selected_index().map(|i| &self.rows[i])
    }

    /// `(path, line)` to jump to for the selected heading.
    pub fn jump_target(&self) -> Option<(&Path, usize)> {
        self.selected().map(|r| (r.file.as_path(), r.line))
    }

    /// Move the cursor down one heading (saturating).
    pub fn move_down(&mut self) {
        if !self.selectable.is_empty() && self.cursor + 1 < self.selectable.len() {
            self.cursor += 1;
        }
    }

    /// Move the cursor up one heading (saturating).
    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
}

// ---- AgendaView --------------------------------------------------------------

/// Which agenda view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgendaMode {
    #[default]
    Today,
    Next7Days,
    TodoList,
}

impl AgendaMode {
    /// Cycle Today → Next-7 → TODO-list → Today.
    pub fn next(self) -> Self {
        match self {
            Self::Today => Self::Next7Days,
            Self::Next7Days => Self::TodoList,
            Self::TodoList => Self::Today,
        }
    }

    /// Display label for the mode header / tab strip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Next7Days => "Next 7 days",
            Self::TodoList => "TODO list",
        }
    }
}

/// What an agenda row represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgendaRowKind {
    /// A day separator in the next-7 view.
    DayHeader,
    /// A file separator in the TODO-list view.
    FileHeader,
    /// A schedulable / TODO item (cursor lands here).
    Item,
}

/// One rendered agenda row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgendaRow {
    pub kind: AgendaRowKind,
    pub label: String,
    /// Jump target file (items only).
    pub file: Option<PathBuf>,
    /// Jump target line (items only).
    pub line: Option<usize>,
    /// `true` for past-due open items (Today view).
    pub overdue: bool,
}

/// The agenda view-model. Holds mode, filter, the computed rows, and a cursor
/// over the item rows. Rebuilds its rows from the root on mode/filter changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgendaView {
    mode: AgendaMode,
    filter_query: String,
    rows: Vec<AgendaRow>,
    selectable: Vec<usize>,
    cursor: usize,
}

impl AgendaView {
    /// Build an agenda in `mode` from `root`, with `today` as the reference day
    /// and an empty filter.
    pub fn build(root: &OrgRoot, today: cockpit_org::OrgDate, mode: AgendaMode) -> Self {
        let mut view = AgendaView {
            mode,
            ..Default::default()
        };
        view.recompute(root, today);
        view
    }

    /// The current mode.
    pub fn mode(&self) -> AgendaMode {
        self.mode
    }

    /// The current filter query string.
    pub fn filter_query(&self) -> &str {
        &self.filter_query
    }

    /// All rows, for the painter.
    pub fn rows(&self) -> &[AgendaRow] {
        &self.rows
    }

    /// Cursor index within the selectable (item) rows.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected item row, if any.
    pub fn selected(&self) -> Option<&AgendaRow> {
        self.selectable.get(self.cursor).map(|&i| &self.rows[i])
    }

    /// `(path, line)` to jump to for the selected item.
    pub fn jump_target(&self) -> Option<(&Path, usize)> {
        self.selected()
            .and_then(|r| Some((r.file.as_deref()?, r.line?)))
    }

    /// Move the cursor down one item (saturating).
    pub fn move_down(&mut self) {
        if !self.selectable.is_empty() && self.cursor + 1 < self.selectable.len() {
            self.cursor += 1;
        }
    }

    /// Move the cursor up one item (saturating).
    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Switch to the next mode and rebuild.
    pub fn cycle_mode(&mut self, root: &OrgRoot, today: cockpit_org::OrgDate) {
        self.mode = self.mode.next();
        self.recompute(root, today);
    }

    /// Replace the filter query and rebuild.
    pub fn set_filter(
        &mut self,
        query: impl Into<String>,
        root: &OrgRoot,
        today: cockpit_org::OrgDate,
    ) {
        self.filter_query = query.into();
        self.recompute(root, today);
    }

    /// Rebuild the rows from the current mode + filter, keeping the cursor in
    /// bounds.
    pub fn recompute(&mut self, root: &OrgRoot, today: cockpit_org::OrgDate) {
        let filter = Filter::parse(&self.filter_query);
        let (rows, selectable) = match self.mode {
            AgendaMode::Today => today_rows(root, today, &filter),
            AgendaMode::Next7Days => next7_rows(root, today, &filter),
            AgendaMode::TodoList => todo_rows(root, &filter),
        };
        self.rows = rows;
        self.selectable = selectable;
        if self.cursor >= self.selectable.len() {
            self.cursor = self.selectable.len().saturating_sub(1);
        }
    }
}

fn today_rows(
    root: &OrgRoot,
    today: cockpit_org::OrgDate,
    filter: &Filter,
) -> (Vec<AgendaRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut selectable = Vec::new();
    for item in agenda_today(root, today, filter) {
        selectable.push(rows.len());
        rows.push(item_row(&item));
    }
    (rows, selectable)
}

fn next7_rows(
    root: &OrgRoot,
    start: cockpit_org::OrgDate,
    filter: &Filter,
) -> (Vec<AgendaRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut selectable = Vec::new();
    for day in next_7_days(root, start, filter) {
        rows.push(AgendaRow {
            kind: AgendaRowKind::DayHeader,
            label: format!(
                "{:04}-{:02}-{:02} {}",
                day.date.year,
                day.date.month,
                day.date.day,
                date::weekday_abbr(day.date)
            ),
            file: None,
            line: None,
            overdue: false,
        });
        for item in &day.items {
            selectable.push(rows.len());
            rows.push(item_row(item));
        }
    }
    (rows, selectable)
}

fn todo_rows(root: &OrgRoot, filter: &Filter) -> (Vec<AgendaRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut selectable = Vec::new();
    for group in todo_list(root, filter) {
        rows.push(AgendaRow {
            kind: AgendaRowKind::FileHeader,
            label: file_name(&group.file),
            file: None,
            line: None,
            overdue: false,
        });
        for item in &group.items {
            selectable.push(rows.len());
            rows.push(item_row(item));
        }
    }
    (rows, selectable)
}

fn item_row(item: &AgendaItem) -> AgendaRow {
    AgendaRow {
        kind: AgendaRowKind::Item,
        label: item_label(item),
        file: Some(item.file.clone()),
        line: Some(item.line),
        overdue: item.overdue,
    }
}

fn item_label(item: &AgendaItem) -> String {
    let mut s = String::new();
    if let Some(t) = item.time {
        s.push_str(&format!("{:02}:{:02} ", t.hour, t.minute));
    }
    match item.kind {
        AgendaKind::Scheduled => s.push_str("Scheduled: "),
        AgendaKind::Deadline => s.push_str("Deadline: "),
        AgendaKind::Todo => {}
    }
    if let Some(k) = &item.todo_keyword {
        s.push_str(k);
        s.push(' ');
    }
    s.push_str(&item.title);
    s
}

// ---- CaptureView -------------------------------------------------------------

/// One template in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTemplateRow {
    pub key: String,
    pub name: String,
}

/// Which step of the capture flow is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapturePhase {
    /// Showing the template picker.
    #[default]
    Picking,
    /// Editing the expanded entry.
    Editing,
}

/// The result of committing a capture, ready for [`cockpit_org::apply_capture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCommit {
    /// Destination file (relative to the org root), from the template target.
    pub file: String,
    /// Where to file the entry.
    pub target: CaptureTarget,
    /// The expanded, user-edited entry plus the cursor offset.
    pub entry: Expansion,
}

/// The capture flow view-model: pick a template, edit one field, commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureView {
    templates: Vec<CaptureTemplate>,
    phase: CapturePhase,
    picked: Option<usize>,
    buffer: String,
    /// Byte offset of the edit cursor within `buffer`.
    cursor: usize,
}

impl CaptureView {
    /// Build from an explicit template list.
    pub fn new(templates: Vec<CaptureTemplate>) -> Self {
        CaptureView {
            templates,
            ..Default::default()
        }
    }

    /// Build from the `[org]` config.
    pub fn from_config(config: &OrgConfig) -> Self {
        Self::new(config.capture.clone())
    }

    /// The current phase.
    pub fn phase(&self) -> CapturePhase {
        self.phase
    }

    /// Rows for the template picker.
    pub fn template_rows(&self) -> Vec<CaptureTemplateRow> {
        self.templates
            .iter()
            .map(|t| CaptureTemplateRow {
                key: t.key.clone(),
                name: t.name.clone(),
            })
            .collect()
    }

    /// The edit buffer (valid in the Editing phase).
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// The edit cursor byte offset.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Pick the template whose key is `key`, expanding it against `now`/`ctx`
    /// and entering the Editing phase with the cursor at the `%?` slot. Returns
    /// `false` if no template has that key.
    pub fn pick(&mut self, key: &str, now: &NowStamp, ctx: &CaptureContext) -> bool {
        let Some(idx) = self.templates.iter().position(|t| t.key == key) else {
            return false;
        };
        let expansion = cockpit_org::expand(&self.templates[idx].template, now, ctx);
        self.buffer = expansion.text;
        // Cursor at the %? slot, else at the end.
        self.cursor = expansion.cursor.unwrap_or(self.buffer.len());
        self.picked = Some(idx);
        self.phase = CapturePhase::Editing;
        true
    }

    /// Insert a string at the cursor.
    pub fn insert_str(&mut self, s: &str) {
        self.buffer.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Insert a single char at the cursor.
    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the char before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.buffer[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.buffer.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    /// Move the cursor one char left.
    pub fn move_left(&mut self) {
        if let Some((i, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.cursor = i;
        }
    }

    /// Move the cursor one char right.
    pub fn move_right(&mut self) {
        if let Some(c) = self.buffer[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    /// Return to the template picker, discarding the buffer.
    pub fn cancel(&mut self) {
        self.phase = CapturePhase::Picking;
        self.picked = None;
        self.buffer.clear();
        self.cursor = 0;
    }

    /// Build the commit payload from the current buffer, if a template is
    /// being edited.
    pub fn commit(&self) -> Option<CaptureCommit> {
        let template = self.picked.and_then(|i| self.templates.get(i))?;
        Some(CaptureCommit {
            file: template.target.file.clone(),
            target: template.target.clone(),
            entry: Expansion {
                text: self.buffer.clone(),
                cursor: Some(self.cursor),
            },
        })
    }
}

// ---- palette commands (M12.7) -----------------------------------------------

/// Org palette command ids and their default leader keybindings.
///
/// The main cockpit registers these with the command registry and binds the
/// leader chords (M8.2 leader path). Schedule / Deadline / Refile are
/// palette-only (no default chord). The actual handlers live in the binary and
/// call into [`cockpit_org`] (`run_capture`, `cycle_todo`, `set_scheduled`,
/// `set_deadline`, agenda view-models).
pub mod commands {
    /// `(command_id, leader_chord)` for the org commands that have a default
    /// binding (plan M12.7).
    pub const CAPTURE: &str = "org.capture";
    pub const AGENDA: &str = "org.agenda";
    pub const JUMP_TO_INBOX: &str = "org.jump_to_inbox";
    pub const TOGGLE_TODO: &str = "org.toggle_todo";
    pub const SCHEDULE: &str = "org.schedule";
    pub const DEADLINE: &str = "org.deadline";
    pub const REFILE: &str = "org.refile";

    /// Every org command id, for palette registration.
    pub const ALL: &[&str] = &[
        CAPTURE,
        AGENDA,
        JUMP_TO_INBOX,
        TOGGLE_TODO,
        SCHEDULE,
        DEADLINE,
        REFILE,
    ];

    /// Default leader keybindings as `(chord, command_id)` pairs (plan M12.7).
    pub fn leader_keybindings() -> &'static [(&'static str, &'static str)] {
        &[
            ("<leader>oc", CAPTURE),
            ("<leader>oa", AGENDA),
            ("<leader>oi", JUMP_TO_INBOX),
            ("<leader>ot", TOGGLE_TODO),
        ]
    }
}

// ---- helpers -----------------------------------------------------------------

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_org::OrgDate;

    const TODAY: OrgDate = OrgDate {
        year: 2026,
        month: 5,
        day: 29,
    };

    fn root() -> OrgRoot {
        OrgRoot::from_files(
            "/org",
            [
                (
                    "/org/tasks.org",
                    "* TODO Today thing\nSCHEDULED: <2026-05-29 Fri>\n\
                     * TODO Overdue\nDEADLINE: <2026-05-20 Wed>\n\
                     * Plain\n",
                ),
                ("/org/notes.org", "* TODO A note\n"),
            ],
        )
    }

    #[test]
    fn list_view_lists_files_and_headings() {
        let view = OrgListView::build(&root());
        // 2 file headers + 4 headings = 6 rows. BTreeMap orders notes.org
        // before tasks.org.
        assert_eq!(view.rows().len(), 6);
        assert!(view.rows()[0].is_file_header);
        assert_eq!(view.rows()[0].label, "notes.org");
        // Cursor starts on the first heading.
        assert_eq!(view.selected().unwrap().label, "A note");
    }

    #[test]
    fn list_view_cursor_skips_file_headers() {
        let mut view = OrgListView::build(&root());
        let labels: Vec<String> = {
            let mut v = Vec::new();
            loop {
                v.push(view.selected().unwrap().label.clone());
                let before = view.selected_index();
                view.move_down();
                if view.selected_index() == before {
                    break;
                }
            }
            v
        };
        assert_eq!(labels, ["A note", "Today thing", "Overdue", "Plain"]);
        // Jump target is the last heading's file ("Plain" in tasks.org).
        let (path, _line) = view.jump_target().unwrap();
        assert_eq!(path, Path::new("/org/tasks.org"));
    }

    #[test]
    fn agenda_today_view_lists_items() {
        let view = AgendaView::build(&root(), TODAY, AgendaMode::Today);
        let labels: Vec<&str> = view.rows().iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Deadline: TODO Overdue", "Scheduled: TODO Today thing"]
        );
        assert!(view.rows()[0].overdue);
        assert_eq!(view.selected().unwrap().label, "Deadline: TODO Overdue");
    }

    #[test]
    fn agenda_cycles_modes() {
        let mut view = AgendaView::build(&root(), TODAY, AgendaMode::Today);
        assert_eq!(view.mode(), AgendaMode::Today);
        view.cycle_mode(&root(), TODAY);
        assert_eq!(view.mode(), AgendaMode::Next7Days);
        // Next-7 has day-header rows.
        assert!(
            view.rows()
                .iter()
                .any(|r| r.kind == AgendaRowKind::DayHeader)
        );
        view.cycle_mode(&root(), TODAY);
        assert_eq!(view.mode(), AgendaMode::TodoList);
        assert!(
            view.rows()
                .iter()
                .any(|r| r.kind == AgendaRowKind::FileHeader)
        );
    }

    #[test]
    fn agenda_filter_narrows_and_keeps_cursor_in_bounds() {
        let mut view = AgendaView::build(&root(), TODAY, AgendaMode::Today);
        view.move_down(); // cursor on second item
        view.set_filter("+nope", &root(), TODAY);
        assert!(view.rows().is_empty());
        assert_eq!(view.selected(), None);
        assert_eq!(view.cursor(), 0);
    }

    #[test]
    fn agenda_next7_cursor_skips_day_headers() {
        let view = AgendaView::build(&root(), TODAY, AgendaMode::Next7Days);
        // Only the scheduled-today item falls in the window; the overdue one
        // (05-20) is before it.
        assert_eq!(
            view.selected().unwrap().label,
            "Scheduled: TODO Today thing"
        );
        let (path, line) = view.jump_target().unwrap();
        assert_eq!(path, Path::new("/org/tasks.org"));
        assert_eq!(line, 0);
    }

    fn capture_config() -> OrgConfig {
        OrgConfig {
            root: None,
            default_todo_keywords: vec!["TODO".into(), "DONE".into()],
            capture: vec![CaptureTemplate {
                key: "t".into(),
                name: "Todo".into(),
                target: CaptureTarget {
                    file: "inbox.org".into(),
                    under: Some("Tasks".into()),
                    datetree: false,
                },
                template: "* TODO %? :inbox:".into(),
            }],
        }
    }

    fn now() -> NowStamp {
        NowStamp::new(
            OrgDate::new(2026, 5, 29),
            cockpit_org::OrgTime::new(9, 0),
            "Fri",
        )
    }

    #[test]
    fn capture_pick_then_edit_then_commit() {
        let mut view = CaptureView::from_config(&capture_config());
        assert_eq!(view.phase(), CapturePhase::Picking);
        assert_eq!(view.template_rows()[0].name, "Todo");

        assert!(view.pick("t", &now(), &CaptureContext::default()));
        assert_eq!(view.phase(), CapturePhase::Editing);
        // Buffer expanded, cursor at the %? slot (after "* TODO ").
        assert_eq!(view.buffer(), "* TODO  :inbox:");
        assert_eq!(view.cursor(), "* TODO ".len());

        view.insert_str("buy milk");
        assert_eq!(view.buffer(), "* TODO buy milk :inbox:");

        let commit = view.commit().unwrap();
        assert_eq!(commit.file, "inbox.org");
        assert_eq!(commit.target.under.as_deref(), Some("Tasks"));
        assert_eq!(commit.entry.text, "* TODO buy milk :inbox:");
    }

    #[test]
    fn capture_unknown_key_is_ignored() {
        let mut view = CaptureView::from_config(&capture_config());
        assert!(!view.pick("z", &now(), &CaptureContext::default()));
        assert_eq!(view.phase(), CapturePhase::Picking);
    }

    #[test]
    fn capture_edit_backspace_and_cursor() {
        let mut view = CaptureView::from_config(&capture_config());
        view.pick("t", &now(), &CaptureContext::default());
        view.insert_char('x');
        view.insert_char('y');
        assert_eq!(view.buffer(), "* TODO xy :inbox:");
        view.backspace();
        assert_eq!(view.buffer(), "* TODO x :inbox:");
        view.move_left();
        view.insert_char('z');
        assert_eq!(view.buffer(), "* TODO zx :inbox:");
    }

    #[test]
    fn org_commands_have_unique_ids_and_leader_bindings() {
        // Ids are unique.
        let mut ids = commands::ALL.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), commands::ALL.len());

        // Every leader binding points at a known command id, all under `o`.
        for (chord, id) in commands::leader_keybindings() {
            assert!(chord.starts_with("<leader>o"), "unexpected chord {chord}");
            assert!(commands::ALL.contains(id), "unknown id {id}");
        }
        assert_eq!(commands::leader_keybindings().len(), 4);
    }

    #[test]
    fn capture_end_to_end_apply() {
        // The commit payload drives cockpit_org::apply_capture.
        let mut view = CaptureView::from_config(&capture_config());
        view.pick("t", &now(), &CaptureContext::default());
        view.insert_str("ship it");
        let commit = view.commit().unwrap();
        let out = cockpit_org::apply_capture("* Tasks\n", &commit.target, &commit.entry, &now());
        assert_eq!(out.source, "* Tasks\n** TODO ship it :inbox:\n");
    }
}
