//! `cockpit-notebook` — Jupytext-style SQL/ggsql notebook view-model
//! (v0.5 M5.2 / M5.3).
//!
//! The file format is deliberately boring (plan §8a):
//!
//! * Plain `.sql` or `.ggsql` files with `-- %% cell` markers separating
//!   cells. Files without any markers are still notebooks — they parse
//!   into a single cell containing the whole document.
//! * Per-cell metadata lives in trailing `-- %% meta: { ... }` lines.
//! * Cells whose body mentions `VISUALISE` / `VISUALIZE` route through
//!   the ggsql engine (M5.1a); everything else routes through DuckDB
//!   (M5.1).
//!
//! Cell results are *not* persisted in the source file — that would
//! corrupt the diff and make notebooks look like JSON. Callers (the
//! notebook UI in `cockpit-ui`) keep the latest [`CellResult`] in
//! memory and may serialise it to a sibling `.cockpit/results/...`
//! cache.
//!
//! Everything in this crate is plain data + pure functions. No I/O, no
//! engine calls — the view-model is the single source of truth for the
//! notebook's structure; execution happens through
//! [`cockpit_sql::SqlEngine`] in the caller.

pub mod parse;
pub mod quarto;

pub use parse::{NotebookParseError, parse_notebook};
pub use quarto::parse_quarto;

use cockpit_sql::{QueryError, QueryResult, statement_targets_ggsql};
use serde::{Deserialize, Serialize};

/// Cell kind drives both the renderer and the engine routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellKind {
    /// Plain SQL — executes against DuckDB (M5.1).
    Sql,
    /// `VISUALISE` cell — executes against ggsql (M5.1a) and renders
    /// the resulting Vega-Lite JSON inline (M5.5).
    Ggsql,
    /// Markdown — never executed; rendered inline by M5.Q2.
    Markdown,
}

impl CellKind {
    /// True when cockpit knows how to *execute* this cell. Markdown and
    /// future unknown kinds return false; the notebook UI shows them but
    /// the engine layer never gets asked.
    pub fn executable(self) -> bool {
        matches!(self, Self::Sql | Self::Ggsql)
    }
}

/// Status of one cell in the notebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellStatus {
    /// Never run since the cell last changed.
    #[default]
    Idle,
    /// Currently running on the engine.
    Running,
    /// Last run succeeded — the latest result is in [`Cell::result`].
    Ok,
    /// Last run failed — the error is in [`Cell::result`].
    Failed,
}

/// Whatever the engine returned from the most recent run of a cell, if
/// any. Errors are normalised into `Err` so the renderer has one branch
/// per direction instead of three.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CellResult {
    /// Rows / Vega-Lite JSON from the last successful run.
    Ok(QueryResult),
    /// Lossy snapshot of the engine error — the original [`QueryError`]
    /// has no `Serialize`, so we collapse it to a short user-facing
    /// string and keep the kind tag for the UI.
    Err {
        /// One-line summary the UI shows.
        message: String,
    },
}

impl CellResult {
    /// Convert a [`Result<QueryResult, QueryError>`] returned by an
    /// engine into the serialisable [`CellResult`] shape. The full
    /// [`QueryError`] still flows through `tracing` for diagnostics.
    pub fn from_engine(result: Result<QueryResult, QueryError>) -> Self {
        match result {
            Ok(query) => Self::Ok(query),
            Err(err) => Self::Err {
                message: err.to_string(),
            },
        }
    }
}

/// One notebook cell — source text plus its latest engine round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// `-- %% meta: { title = "..." }` — optional human-readable label.
    pub title: Option<String>,
    /// Routing / rendering kind.
    pub kind: CellKind,
    /// Raw cell body (no markers, no trailing meta line). Always
    /// preserved verbatim — trailing newlines included — so round-tripping
    /// a notebook through [`render_notebook`] is byte-identical.
    pub source: String,
    /// Latest run state.
    pub status: CellStatus,
    /// Latest engine result, if any.
    pub result: Option<CellResult>,
}

impl Cell {
    /// New idle cell with no result yet.
    pub fn new(kind: CellKind, source: impl Into<String>) -> Self {
        Self {
            title: None,
            kind,
            source: source.into(),
            status: CellStatus::Idle,
            result: None,
        }
    }

    /// Build a cell whose kind is inferred from its body (`VISUALISE`
    /// targets ggsql; everything else is SQL).
    pub fn sql(source: impl Into<String>) -> Self {
        let source = source.into();
        let kind = if statement_targets_ggsql(&source) {
            CellKind::Ggsql
        } else {
            CellKind::Sql
        };
        Self::new(kind, source)
    }

    /// Set the human-readable title — chainable for tests.
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Record the outcome of an engine call. The status moves to
    /// [`CellStatus::Ok`] on success and [`CellStatus::Failed`] on error
    /// so the UI does not need to peek inside the result to colour the
    /// gutter badge.
    pub fn apply_result(&mut self, result: Result<QueryResult, QueryError>) {
        self.status = if result.is_ok() {
            CellStatus::Ok
        } else {
            CellStatus::Failed
        };
        self.result = Some(CellResult::from_engine(result));
    }

    /// Mark the cell as running. Caller resets to `Ok`/`Failed` via
    /// [`apply_result`](Self::apply_result) when the engine returns.
    pub fn mark_running(&mut self) {
        self.status = CellStatus::Running;
    }
}

/// Full notebook — an ordered list of cells plus the cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notebook {
    pub cells: Vec<Cell>,
    /// Index of the cell currently under the user's focus. Saturates to
    /// the last cell when the notebook shrinks.
    pub active: usize,
}

impl Notebook {
    /// New notebook with a single empty SQL cell.
    pub fn new() -> Self {
        Self {
            cells: vec![Cell::sql(String::new())],
            active: 0,
        }
    }

    /// Notebook with the given cells. Defaults the cursor to the first
    /// cell when `cells` is non-empty; empty notebooks fall back to one
    /// blank SQL cell so the view-model is always non-empty.
    pub fn from_cells(cells: Vec<Cell>) -> Self {
        if cells.is_empty() {
            return Self::new();
        }
        Self { cells, active: 0 }
    }

    /// Move the cursor down one cell (saturates at the last cell).
    pub fn move_down(&mut self) {
        if self.active + 1 < self.cells.len() {
            self.active += 1;
        }
    }

    /// Move the cursor up one cell (saturates at 0).
    pub fn move_up(&mut self) {
        self.active = self.active.saturating_sub(1);
    }

    /// Cell under the cursor, if any.
    pub fn active_cell(&self) -> Option<&Cell> {
        self.cells.get(self.active)
    }

    /// Mutable reference to the active cell — the engine wrapper uses
    /// this to mark running / apply results.
    pub fn active_cell_mut(&mut self) -> Option<&mut Cell> {
        self.cells.get_mut(self.active)
    }

    /// Insert a fresh empty SQL cell *after* the active cell and move
    /// the cursor onto it. The new cell is always `CellKind::Sql` —
    /// users can convert it to a ggsql cell by typing `VISUALISE`.
    pub fn insert_cell_below(&mut self) {
        let insert_at = (self.active + 1).min(self.cells.len());
        self.cells.insert(insert_at, Cell::sql(String::new()));
        self.active = insert_at;
    }

    /// Remove the active cell. The notebook always keeps at least one
    /// cell — removing the last one resets it to an empty SQL cell.
    pub fn remove_active_cell(&mut self) {
        if self.cells.len() <= 1 {
            self.cells = vec![Cell::sql(String::new())];
            self.active = 0;
            return;
        }
        self.cells.remove(self.active);
        if self.active >= self.cells.len() {
            self.active = self.cells.len() - 1;
        }
    }

    /// Replace the active cell's body. Re-evaluates the kind so a user
    /// typing `VISUALISE …` converts the cell to a ggsql cell on the
    /// next render. Clears the cached result — stale rows would lie.
    pub fn set_active_source(&mut self, source: impl Into<String>) {
        let Some(cell) = self.active_cell_mut() else {
            return;
        };
        let source = source.into();
        let kind = if statement_targets_ggsql(&source) {
            CellKind::Ggsql
        } else if cell.kind == CellKind::Markdown {
            CellKind::Markdown
        } else {
            CellKind::Sql
        };
        cell.source = source;
        cell.kind = kind;
        cell.status = CellStatus::Idle;
        cell.result = None;
    }
}

impl Default for Notebook {
    fn default() -> Self {
        Self::new()
    }
}

/// True when `path` is a Jupytext-style notebook source — used by the
/// editor pane to switch from a plain text editor to the notebook view.
pub fn is_notebook_source(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("-- %%"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_sql::SqlValue;

    #[test]
    fn cell_sql_infers_ggsql_kind_for_visualise_bodies() {
        assert_eq!(Cell::sql("SELECT 1").kind, CellKind::Sql);
        assert_eq!(
            Cell::sql("SELECT x VISUALISE DRAW point").kind,
            CellKind::Ggsql
        );
    }

    #[test]
    fn apply_result_moves_status_and_stores_outcome() {
        let mut cell = Cell::sql("SELECT 1");
        cell.apply_result(Ok(QueryResult {
            columns: vec!["n".to_string()],
            rows: vec![vec![SqlValue::Integer(1)]],
            elapsed_ms: None,
        }));
        assert_eq!(cell.status, CellStatus::Ok);
        assert!(matches!(cell.result, Some(CellResult::Ok(_))));

        let mut cell = Cell::sql("SELECT bad");
        cell.apply_result(Err(QueryError::EngineError {
            stderr_excerpt: "boom".to_string(),
        }));
        assert_eq!(cell.status, CellStatus::Failed);
        assert!(matches!(cell.result, Some(CellResult::Err { .. })));
    }

    #[test]
    fn notebook_navigation_saturates_at_the_bounds() {
        let mut nb = Notebook::from_cells(vec![Cell::sql("a"), Cell::sql("b"), Cell::sql("c")]);
        assert_eq!(nb.active, 0);
        nb.move_up();
        assert_eq!(nb.active, 0);
        nb.move_down();
        nb.move_down();
        nb.move_down();
        assert_eq!(nb.active, 2);
    }

    #[test]
    fn insert_cell_below_advances_to_the_new_cell() {
        let mut nb = Notebook::from_cells(vec![Cell::sql("a")]);
        nb.insert_cell_below();
        assert_eq!(nb.cells.len(), 2);
        assert_eq!(nb.active, 1);
        assert_eq!(nb.cells[1].source, "");
    }

    #[test]
    fn remove_active_cell_keeps_the_notebook_non_empty() {
        let mut nb = Notebook::from_cells(vec![Cell::sql("only")]);
        nb.remove_active_cell();
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].source, "");
        assert_eq!(nb.active, 0);
    }

    #[test]
    fn set_active_source_promotes_a_sql_cell_to_ggsql() {
        let mut nb = Notebook::from_cells(vec![Cell::sql("SELECT 1")]);
        // Seed a result so we can prove it gets cleared.
        nb.cells[0].apply_result(Ok(QueryResult::empty()));
        assert_eq!(nb.cells[0].status, CellStatus::Ok);

        nb.set_active_source("SELECT x VISUALISE DRAW point");
        let cell = &nb.cells[0];
        assert_eq!(cell.kind, CellKind::Ggsql);
        assert_eq!(cell.status, CellStatus::Idle);
        assert!(cell.result.is_none(), "old result must be cleared");
    }

    #[test]
    fn markdown_cells_stay_markdown_after_edits() {
        let mut nb = Notebook::from_cells(vec![Cell::new(CellKind::Markdown, "# heading")]);
        nb.set_active_source("# new heading\nbody");
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
    }

    #[test]
    fn is_notebook_source_detects_the_cell_marker() {
        assert!(is_notebook_source("-- %% cell\nSELECT 1\n"));
        assert!(!is_notebook_source("SELECT 1\nSELECT 2\n"));
    }
}
