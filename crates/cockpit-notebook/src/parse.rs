//! Jupytext-style parser for `.sql` / `.ggsql` / `.qmd` files
//! (v0.5 M5.2 / M5.Q1).
//!
//! Two markers:
//!
//! * `-- %% cell` (with an optional trailing `kind = sql` /
//!   `kind = ggsql` / `kind = markdown` annotation) starts a new cell.
//! * `-- %% meta: { ... }` attaches simple key=value metadata to the
//!   preceding cell. The body is a flat list of `key = "value"` pairs;
//!   we keep this minimal on purpose — full KDL only lands if the
//!   notebook UI needs it.
//!
//! Files without any markers parse into a single cell containing the
//! whole document — opening a plain `.sql` file should not lose data
//! when it gets routed through the notebook view-model.

use thiserror::Error;

use crate::{Cell, CellKind, Notebook};

/// Things the parser can complain about. The header parser is permissive
/// on purpose; only outright malformed markers raise an error so a
/// human-written notebook is hard to break by accident.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NotebookParseError {
    /// `-- %% cell kind = …` referenced an unknown kind.
    #[error("unknown cell kind `{0}`")]
    UnknownKind(String),
}

/// Parse `source` into a [`Notebook`]. `default_kind` is the cell kind
/// applied when a cell does not carry an explicit `kind = ...`
/// annotation — `.sql` files default to [`CellKind::Sql`], `.ggsql`
/// files to [`CellKind::Ggsql`], `.qmd` files to [`CellKind::Markdown`].
pub fn parse_notebook(
    source: &str,
    default_kind: CellKind,
) -> Result<Notebook, NotebookParseError> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut current_kind = default_kind;
    let mut current_title: Option<String> = None;
    let mut buffer: Vec<&str> = Vec::new();

    let flush =
        |cells: &mut Vec<Cell>, kind: CellKind, title: Option<String>, buffer: &Vec<&str>| {
            if buffer.is_empty() && cells.is_empty() {
                // Don't create a leading empty cell when the file starts
                // with a `-- %% cell` marker.
                return;
            }
            let body = buffer.join("\n");
            let mut cell = if matches!(kind, CellKind::Markdown) {
                Cell::new(kind, body)
            } else {
                let mut cell = Cell::sql(body);
                // Honour an explicit annotation when one was set —
                // `Cell::sql` only routes by body content.
                cell.kind = match kind {
                    CellKind::Sql => cell.kind,
                    other => other,
                };
                cell
            };
            cell.title = title;
            cells.push(cell);
        };

    for line in source.lines() {
        let trimmed = line.trim_start();
        if let Some(annotation) = trimmed.strip_prefix("-- %% cell") {
            flush(&mut cells, current_kind, current_title.take(), &buffer);
            buffer.clear();
            current_kind = parse_cell_header(annotation, default_kind)?;
            current_title = None;
            continue;
        }
        if let Some(meta) = trimmed.strip_prefix("-- %% meta:") {
            if let Some(title) = parse_inline_title(meta) {
                if let Some(last) = cells.last_mut() {
                    last.title = Some(title);
                } else {
                    current_title = Some(title);
                }
            }
            continue;
        }
        buffer.push(line);
    }
    flush(&mut cells, current_kind, current_title, &buffer);

    Ok(Notebook::from_cells(cells))
}

fn parse_cell_header(
    annotation: &str,
    default_kind: CellKind,
) -> Result<CellKind, NotebookParseError> {
    // Accept any whitespace-trimmed suffix: `[blank]`, `kind = sql`,
    // `kind=ggsql`, `kind = markdown`. Anything else is a parse error so
    // we surface typos instead of silently defaulting.
    let trimmed = annotation.trim();
    if trimmed.is_empty() {
        return Ok(default_kind);
    }
    let Some(rest) = trimmed.strip_prefix("kind") else {
        return Err(NotebookParseError::UnknownKind(trimmed.to_string()));
    };
    let value = rest.trim_start().trim_start_matches('=').trim();
    match value {
        "sql" => Ok(CellKind::Sql),
        "ggsql" => Ok(CellKind::Ggsql),
        "markdown" | "md" => Ok(CellKind::Markdown),
        other => Err(NotebookParseError::UnknownKind(other.to_string())),
    }
}

/// Extract the `title` field from a `-- %% meta: { title = "Foo" }`
/// payload. The parser ignores any other keys for now — they round-trip
/// through the source untouched because we never serialise back into
/// the file.
fn parse_inline_title(meta: &str) -> Option<String> {
    let trimmed = meta.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(trimmed);
    for pair in inner.split(',') {
        let pair = pair.trim();
        if let Some(rest) = pair.strip_prefix("title") {
            let value = rest.trim_start().trim_start_matches('=').trim();
            let value = value.trim_matches('"');
            if value.is_empty() {
                return None;
            }
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_without_markers_yields_one_cell_with_the_whole_body() {
        let nb = parse_notebook("SELECT 1\nSELECT 2\n", CellKind::Sql).unwrap();
        assert_eq!(nb.cells.len(), 1);
        // `lines()` strips the trailing newline; we round-trip the
        // body content but not the final blank line. The notebook UI
        // re-adds a trailing newline on save when appropriate.
        assert_eq!(nb.cells[0].source, "SELECT 1\nSELECT 2");
        assert_eq!(nb.cells[0].kind, CellKind::Sql);
    }

    #[test]
    fn leading_marker_does_not_emit_a_blank_first_cell() {
        let source = "-- %% cell\nSELECT 1\n-- %% cell\nSELECT 2\n";
        let nb = parse_notebook(source, CellKind::Sql).unwrap();
        assert_eq!(nb.cells.len(), 2);
        assert_eq!(nb.cells[0].source, "SELECT 1");
        assert_eq!(nb.cells[1].source, "SELECT 2");
    }

    #[test]
    fn explicit_kind_annotation_wins_over_the_default() {
        let source = "\
SELECT 1
-- %% cell kind = markdown
# heading
body
-- %% cell kind=ggsql
SELECT x FROM t VISUALISE DRAW point
";
        let nb = parse_notebook(source, CellKind::Sql).unwrap();
        assert_eq!(nb.cells.len(), 3);
        assert_eq!(nb.cells[0].kind, CellKind::Sql);
        assert_eq!(nb.cells[1].kind, CellKind::Markdown);
        // Body says VISUALISE; explicit annotation also says ggsql.
        assert_eq!(nb.cells[2].kind, CellKind::Ggsql);
        // The first cell's source includes only the body before the marker.
        assert_eq!(nb.cells[0].source, "SELECT 1");
        assert_eq!(nb.cells[1].source, "# heading\nbody");
    }

    #[test]
    fn unknown_kind_annotation_is_a_parse_error() {
        let err = parse_notebook("-- %% cell kind = banana\nSELECT 1", CellKind::Sql).unwrap_err();
        assert!(matches!(err, NotebookParseError::UnknownKind(name) if name == "banana"));
    }

    #[test]
    fn meta_line_attaches_a_title_to_the_preceding_cell() {
        let source = "\
-- %% cell
SELECT 1 AS n
-- %% meta: { title = \"first cell\" }
-- %% cell
SELECT 2
";
        let nb = parse_notebook(source, CellKind::Sql).unwrap();
        assert_eq!(nb.cells[0].title.as_deref(), Some("first cell"));
        assert_eq!(nb.cells[1].title, None);
    }

    #[test]
    fn visualise_in_default_kind_still_routes_to_ggsql() {
        let source = "SELECT * FROM t VISUALISE DRAW point";
        let nb = parse_notebook(source, CellKind::Sql).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Ggsql);
    }
}
