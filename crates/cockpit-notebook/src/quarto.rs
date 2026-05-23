//! Quarto `.qmd` parser — v0.5 M5.Q1.
//!
//! Treats `.qmd` files as a peer notebook format: Markdown narrative with
//! fenced code chunks. The chunk header `{lang}` (with optional `#| key:
//! value` options below the fence) maps to a [`CellKind`]:
//!
//! * `{sql}` → [`CellKind::Sql`] (or [`CellKind::Ggsql`] when the body
//!   mentions `VISUALISE`).
//! * `{ggsql}` → [`CellKind::Ggsql`].
//! * `{python}`, `{r}`, anything else → still parsed as a cell so the
//!   UI can render and label it, but cockpit refuses to execute it. The
//!   cell kind is [`CellKind::Markdown`] for now (a future v0.6+ kernel
//!   layer would promote it); the original language tag survives in
//!   the cell title so renderers can show a "language unsupported"
//!   banner without losing the user's intent.
//!
//! Non-code prose between fences becomes a [`CellKind::Markdown`] cell.

use crate::{Cell, CellKind, Notebook, NotebookParseError};

/// Parse a Quarto `.qmd` document into the same [`Notebook`] view-model
/// used by M5.2's Jupytext parser. Errors are deliberately the same
/// [`NotebookParseError`] so the notebook view-model has one error type
/// to surface.
pub fn parse_quarto(source: &str) -> Result<Notebook, NotebookParseError> {
    let mut cells: Vec<Cell> = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut in_chunk = false;
    let mut chunk_kind = CellKind::Sql;
    let mut chunk_lang_tag: Option<String> = None;
    let mut chunk_label: Option<String> = None;
    let mut chunk_body: Vec<&str> = Vec::new();

    for line in source.lines() {
        let trimmed = line.trim_start();
        if !in_chunk {
            if let Some(rest) = trimmed.strip_prefix("```") {
                let header = rest.trim();
                let Some(language) = parse_chunk_header(header)? else {
                    // Plain ``` opens a non-Quarto code block — let it
                    // stay in the prose buffer so Markdown rendering can
                    // handle the fence itself.
                    prose.push(line);
                    continue;
                };
                // Flush any accumulated prose into a Markdown cell.
                flush_prose(&mut cells, &mut prose);
                let (kind, executable_tag) = classify_language(&language);
                chunk_kind = kind;
                chunk_lang_tag = if executable_tag { None } else { Some(language) };
                chunk_label = None;
                chunk_body.clear();
                in_chunk = true;
                continue;
            }
            prose.push(line);
            continue;
        }
        // Inside a chunk.
        if trimmed.starts_with("```") {
            let kind = chunk_kind;
            let lang_tag = chunk_lang_tag.take();
            let label = chunk_label.take();
            let body = chunk_body.join("\n");
            let cell = match kind {
                CellKind::Markdown if lang_tag.is_some() => {
                    let tag = lang_tag.unwrap_or_default();
                    let title = label
                        .clone()
                        .map(|l| format!("{tag} — {l}"))
                        .unwrap_or(format!("{tag} (unsupported)"));
                    Cell::new(CellKind::Markdown, body).with_title(title)
                }
                CellKind::Sql => {
                    let mut cell = Cell::sql(body);
                    if let Some(label) = label {
                        cell = cell.with_title(label);
                    }
                    cell
                }
                other_kind => {
                    let mut cell = Cell::new(other_kind, body);
                    if let Some(label) = label {
                        cell = cell.with_title(label);
                    }
                    cell
                }
            };
            cells.push(cell);
            in_chunk = false;
            continue;
        }
        if let Some(option) = trimmed.strip_prefix("#|") {
            // Per Quarto's chunk-option syntax: `#| label: foo` /
            // `#| echo: false`. We only extract `label` for now; other
            // options are recognised but ignored.
            if let Some(label) = parse_label_option(option) {
                chunk_label = Some(label);
            }
            continue;
        }
        chunk_body.push(line);
    }
    if in_chunk {
        // Unclosed fence — treat the rest as prose so the user does not
        // lose any text.
        prose.extend(chunk_body.iter().copied());
    }
    flush_prose(&mut cells, &mut prose);

    Ok(Notebook::from_cells(cells))
}

fn flush_prose(cells: &mut Vec<Cell>, prose: &mut Vec<&str>) {
    let trimmed: Vec<&str> = prose
        .iter()
        .copied()
        .skip_while(|line| line.trim().is_empty())
        .collect();
    let has_content = trimmed.iter().any(|line| !line.trim().is_empty());
    prose.clear();
    if !has_content {
        return;
    }
    let body = trimmed.join("\n");
    let body = body.trim_end().to_string();
    cells.push(Cell::new(CellKind::Markdown, body));
}

fn parse_chunk_header(header: &str) -> Result<Option<String>, NotebookParseError> {
    let trimmed = header.trim();
    if !trimmed.starts_with('{') {
        return Ok(None);
    }
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| NotebookParseError::UnknownKind(trimmed.to_string()))?;
    // The chunk header may carry options after the language, e.g.
    // `{sql connection=conn}`. We only care about the language token.
    let language = inner
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if language.is_empty() {
        return Err(NotebookParseError::UnknownKind(trimmed.to_string()));
    }
    Ok(Some(language))
}

fn classify_language(language: &str) -> (CellKind, bool) {
    match language {
        "sql" => (CellKind::Sql, true),
        "ggsql" => (CellKind::Ggsql, true),
        // Anything else parses as a Markdown cell so the UI can show
        // it; cockpit refuses to execute it for now (M5.Q1: explicit
        // out-of-scope).
        _ => (CellKind::Markdown, false),
    }
}

fn parse_label_option(option: &str) -> Option<String> {
    let trimmed = option.trim();
    let rest = trimmed.strip_prefix("label")?;
    let value = rest.trim_start().trim_start_matches(':').trim();
    let value = value.trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_only_qmd_yields_a_single_markdown_cell() {
        let nb = parse_quarto("# heading\n\nbody text\n").unwrap();
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
        assert!(nb.cells[0].source.contains("heading"));
    }

    #[test]
    fn sql_chunk_becomes_a_sql_cell() {
        let source = "\
intro
```{sql}
SELECT 1 AS n
```
outro
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells.len(), 3);
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
        assert_eq!(nb.cells[1].kind, CellKind::Sql);
        assert_eq!(nb.cells[1].source, "SELECT 1 AS n");
        assert_eq!(nb.cells[2].kind, CellKind::Markdown);
    }

    #[test]
    fn ggsql_chunk_becomes_a_ggsql_cell() {
        let source = "\
```{ggsql}
SELECT x VISUALISE DRAW point
```
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Ggsql);
    }

    #[test]
    fn sql_chunk_with_visualise_body_routes_to_ggsql() {
        let source = "\
```{sql}
SELECT x VISUALISE DRAW point
```
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Ggsql);
    }

    #[test]
    fn label_option_becomes_the_cell_title() {
        let source = "\
```{sql}
#| label: first-cell
SELECT 1
```
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells[0].title.as_deref(), Some("first-cell"));
        assert_eq!(nb.cells[0].source, "SELECT 1");
    }

    #[test]
    fn unsupported_language_round_trips_as_a_markdown_cell_with_tag() {
        let source = "\
```{python}
print('hi')
```
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
        assert!(
            nb.cells[0]
                .title
                .as_deref()
                .is_some_and(|t| t.contains("python")),
            "title: {:?}",
            nb.cells[0].title,
        );
        // The user's code survives so save round-trips don't lose data.
        assert!(nb.cells[0].source.contains("print('hi')"));
    }

    #[test]
    fn unclosed_fence_does_not_lose_text() {
        let source = "\
```{sql}
SELECT 1
SELECT 2
";
        let nb = parse_quarto(source).unwrap();
        // The opened fence with no closer becomes prose so the user can
        // see what they typed and fix the missing `\`\`\``.
        let combined: String = nb.cells.iter().map(|c| c.source.clone()).collect();
        assert!(combined.contains("SELECT 1"));
        assert!(combined.contains("SELECT 2"));
    }

    #[test]
    fn plain_triple_backticks_pass_through_as_prose() {
        let source = "\
```
not a Quarto chunk
```
";
        let nb = parse_quarto(source).unwrap();
        assert_eq!(nb.cells[0].kind, CellKind::Markdown);
        assert!(nb.cells[0].source.contains("not a Quarto chunk"));
    }
}
