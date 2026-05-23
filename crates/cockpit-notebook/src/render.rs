//! Inline result + Markdown rendering view-models (M5.4 / M5.Q2).
//!
//! Everything that lands in the editor pane beneath a cell is computed
//! here, so the actual painter in `cockpit-render` reads pixel-free
//! data and the unit tests can assert without a window (spec §18.8).
//!
//! Two surfaces:
//!
//! * [`TableView`] — pure data tier for the virtualised grid that
//!   shows DuckDB rows. Caller picks a viewport (first row + visible
//!   row count) and reads back the bounded slice plus the column
//!   header. No allocations for unrendered rows.
//! * [`MarkdownBlock`] — pure pulldown-cmark-style segmentation of a
//!   Markdown source string into headings, paragraphs, lists, code
//!   spans, and code blocks. The cockpit-ui renderer takes these
//!   blocks and emits the actual text runs in M5.Q2's painter.
//!
//! Charts (M5.5) work on the same `vega_lite` JSON column ggsql
//! produces — there's no extra view-model for them today; the painter
//! reaches for `GgsqlEngine::extract_vega_lite` and pipes the result
//! to `vl-convert`. That decision lives in the binary; we keep this
//! crate engine-agnostic.

use cockpit_sql::{QueryResult, SqlValue};

/// Headless slice of a query result for the virtualised grid. The
/// caller picks the viewport; we slice out the relevant rows and
/// pre-format every cell so the painter never reaches into [`SqlValue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableView<'a> {
    /// Column header in declaration order.
    pub columns: &'a [String],
    /// Visible rows, already display-formatted. Indices are local to
    /// the slice; add `first` back for the original row index.
    pub rows: Vec<Vec<String>>,
    /// Index of the first visible row in the underlying result.
    pub first: usize,
    /// Total row count — drives the scroll-bar widget.
    pub total: usize,
}

impl<'a> TableView<'a> {
    /// New view spanning at most `visible` rows starting at `first`.
    /// Saturates the start at the last row and the visible count at the
    /// remaining rows so the slice is always in range.
    pub fn new(result: &'a QueryResult, first: usize, visible: usize) -> Self {
        let total = result.rows.len();
        let first = first.min(total);
        let end = (first + visible).min(total);
        let rows = result.rows[first..end]
            .iter()
            .map(|row| row.iter().map(SqlValue::display).collect())
            .collect();
        Self {
            columns: &result.columns,
            rows,
            first,
            total,
        }
    }

    /// True when the result has zero rows — the painter swaps to a
    /// "0 rows" placeholder.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// One Markdown segment the painter emits as a single text run (or
/// fenced block, for [`MarkdownBlock::CodeBlock`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownBlock {
    /// A heading at the given level (1–6 in CommonMark).
    Heading { level: u8, text: String },
    /// A paragraph of inline text. Bold/italic spans are recorded but
    /// not nested — the renderer reads them in order.
    Paragraph { inlines: Vec<MarkdownInline> },
    /// One bullet-list item; lists themselves are a sequence of these
    /// produced by [`parse_markdown`].
    ListItem { inlines: Vec<MarkdownInline> },
    /// A fenced code block. `language` is the info-string (may be
    /// empty for plain ``` blocks). Body is unprocessed.
    CodeBlock { language: String, body: String },
}

/// One inline run inside a [`MarkdownBlock::Paragraph`] or
/// [`MarkdownBlock::ListItem`]. The painter applies bold/italic by
/// switching font weight/style for the matching run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownInline {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
}

/// Parse `source` into a flat sequence of blocks. Deliberately
/// permissive — anything we don't recognise becomes a plain paragraph
/// so the user's text never disappears.
///
/// Only the subset called out in plan §8a M5.Q2 lands in v0.5:
/// headings, emphasis, lists, code spans, fenced code blocks. Tables,
/// footnotes, and inline HTML are explicit non-goals.
pub fn parse_markdown(source: &str) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<&str> = Vec::new();
    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_body: Vec<&str> = Vec::new();

    let flush_paragraph = |blocks: &mut Vec<MarkdownBlock>, paragraph: &mut Vec<&str>| {
        if paragraph.is_empty() {
            return;
        }
        let joined = paragraph.join("\n");
        let inlines = parse_inlines(&joined);
        if !inlines.is_empty() {
            blocks.push(MarkdownBlock::Paragraph { inlines });
        }
        paragraph.clear();
    };

    for line in source.lines() {
        let trimmed = line.trim();
        // Fenced code blocks always toggle, even mid-paragraph — that
        // matches CommonMark's "fences end any open paragraph" rule.
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_code {
                blocks.push(MarkdownBlock::CodeBlock {
                    language: std::mem::take(&mut code_lang),
                    body: code_body.join("\n"),
                });
                code_body.clear();
                in_code = false;
            } else {
                flush_paragraph(&mut blocks, &mut paragraph);
                code_lang = rest.trim().to_string();
                in_code = true;
            }
            continue;
        }
        if in_code {
            code_body.push(line);
            continue;
        }
        if trimmed.is_empty() {
            flush_paragraph(&mut blocks, &mut paragraph);
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            let text = trimmed.trim_start_matches('#').trim().to_string();
            blocks.push(MarkdownBlock::Heading { level, text });
            continue;
        }
        if let Some(item) = list_item_body(trimmed) {
            flush_paragraph(&mut blocks, &mut paragraph);
            let inlines = parse_inlines(item);
            blocks.push(MarkdownBlock::ListItem { inlines });
            continue;
        }
        paragraph.push(line);
    }
    if in_code {
        // Unclosed fence: spill the body into a code block anyway so
        // we don't lose the user's text.
        blocks.push(MarkdownBlock::CodeBlock {
            language: code_lang,
            body: code_body.join("\n"),
        });
    } else {
        flush_paragraph(&mut blocks, &mut paragraph);
    }
    blocks
}

fn heading_level(line: &str) -> Option<u8> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    // Must be followed by a space to count as a heading.
    if line.chars().nth(hashes) == Some(' ') {
        Some(hashes as u8)
    } else {
        None
    }
}

fn list_item_body(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("- ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("* ") {
        return Some(rest);
    }
    None
}

fn parse_inlines(source: &str) -> Vec<MarkdownInline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                if !buf.is_empty() {
                    out.push(MarkdownInline::Text(std::mem::take(&mut buf)));
                }
                let mut code = String::new();
                for c in chars.by_ref() {
                    if c == '`' {
                        break;
                    }
                    code.push(c);
                }
                if !code.is_empty() {
                    out.push(MarkdownInline::Code(code));
                }
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if !buf.is_empty() {
                    out.push(MarkdownInline::Text(std::mem::take(&mut buf)));
                }
                let bold = take_until_double_star(&mut chars);
                if !bold.is_empty() {
                    out.push(MarkdownInline::Bold(bold));
                }
            }
            '*' | '_' => {
                if !buf.is_empty() {
                    out.push(MarkdownInline::Text(std::mem::take(&mut buf)));
                }
                let delim = c;
                let mut italic = String::new();
                for next in chars.by_ref() {
                    if next == delim {
                        break;
                    }
                    italic.push(next);
                }
                if !italic.is_empty() {
                    out.push(MarkdownInline::Italic(italic));
                }
            }
            other => buf.push(other),
        }
    }
    if !buf.is_empty() {
        out.push(MarkdownInline::Text(buf));
    }
    out
}

fn take_until_double_star(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            return out;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_sql::SqlValue;

    fn result_with_rows(rows: usize) -> QueryResult {
        QueryResult {
            columns: vec!["n".to_string(), "label".to_string()],
            rows: (0..rows)
                .map(|i| {
                    vec![
                        SqlValue::Integer(i as i64),
                        SqlValue::String(format!("row-{i}")),
                    ]
                })
                .collect(),
            elapsed_ms: None,
        }
    }

    #[test]
    fn table_view_slices_the_visible_window() {
        let result = result_with_rows(10);
        let view = TableView::new(&result, 3, 4);
        assert_eq!(view.first, 3);
        assert_eq!(view.total, 10);
        assert_eq!(view.rows.len(), 4);
        assert_eq!(view.rows[0][0], "3");
        assert_eq!(view.rows[3][1], "row-6");
    }

    #[test]
    fn table_view_clamps_to_the_underlying_row_count() {
        let result = result_with_rows(2);
        let view = TableView::new(&result, 5, 4);
        assert_eq!(view.first, 2);
        assert!(view.rows.is_empty());
    }

    #[test]
    fn table_view_is_empty_when_result_has_no_rows() {
        let empty = QueryResult::empty();
        let view = TableView::new(&empty, 0, 10);
        assert!(view.is_empty());
    }

    #[test]
    fn markdown_headings_capture_level_and_text() {
        let blocks = parse_markdown("# title\n\n## subtitle\n");
        assert_eq!(
            blocks,
            vec![
                MarkdownBlock::Heading {
                    level: 1,
                    text: "title".to_string()
                },
                MarkdownBlock::Heading {
                    level: 2,
                    text: "subtitle".to_string()
                },
            ]
        );
    }

    #[test]
    fn markdown_paragraphs_capture_inline_emphasis() {
        let blocks = parse_markdown("hello **world** and *kind* `code`\n");
        let inlines = match &blocks[0] {
            MarkdownBlock::Paragraph { inlines } => inlines.clone(),
            other => panic!("unexpected: {other:?}"),
        };
        assert!(inlines.contains(&MarkdownInline::Bold("world".to_string())));
        assert!(inlines.contains(&MarkdownInline::Italic("kind".to_string())));
        assert!(inlines.contains(&MarkdownInline::Code("code".to_string())));
    }

    #[test]
    fn markdown_list_items_are_one_block_each() {
        let blocks = parse_markdown("- alpha\n- beta\n");
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], MarkdownBlock::ListItem { .. }));
        assert!(matches!(blocks[1], MarkdownBlock::ListItem { .. }));
    }

    #[test]
    fn markdown_fenced_code_block_preserves_language_and_body() {
        let blocks = parse_markdown("```sql\nSELECT 1\n```\n");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::CodeBlock {
                language: "sql".to_string(),
                body: "SELECT 1".to_string(),
            }]
        );
    }

    #[test]
    fn unterminated_fence_does_not_lose_text() {
        let blocks = parse_markdown("```\nleft hanging");
        assert_eq!(
            blocks,
            vec![MarkdownBlock::CodeBlock {
                language: String::new(),
                body: "left hanging".to_string(),
            }]
        );
    }
}
