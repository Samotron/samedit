//! Tree-sitter syntax highlighting (spec §23 v0.2 / M2.5).
//!
//! [`compute`] turns buffer text into a flat list of [`HighlightSpan`]s that the
//! renderer paints in themed colours. It is a pure function — no I/O, no UI —
//! so token spans are golden-testable (spec §18.3). Large files skip
//! highlighting entirely, satisfying the spec §15 large-file degradation rule.

use std::cell::RefCell;
use std::ops::Range;
use std::path::Path;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Files larger than this skip highlighting (spec §15 large-file mode).
pub const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;

/// A syntax token category. The renderer maps each to a theme colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Comment,
    Constant,
    Variable,
    Operator,
    Attribute,
    Punctuation,
}

/// A highlighted byte range within the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// A source language with tree-sitter support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
}

impl Language {
    /// Pick a language from a file extension (case-insensitive), if supported.
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    /// Pick a language from a file path's extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(Self::from_extension)
    }
}

/// Recognised tree-sitter capture names and the kind each maps to. Dotted query
/// captures (`function.macro`, `type.builtin`, …) fold into the coarse name by
/// tree-sitter's longest-prefix matching, so only the prefixes are listed.
const CAPTURES: &[(&str, HighlightKind)] = &[
    ("attribute", HighlightKind::Attribute),
    ("comment", HighlightKind::Comment),
    ("constant", HighlightKind::Constant),
    ("constructor", HighlightKind::Function),
    ("escape", HighlightKind::String),
    ("function", HighlightKind::Function),
    ("keyword", HighlightKind::Keyword),
    ("label", HighlightKind::Keyword),
    ("operator", HighlightKind::Operator),
    ("property", HighlightKind::Variable),
    ("punctuation", HighlightKind::Punctuation),
    ("string", HighlightKind::String),
    ("type", HighlightKind::Type),
    ("variable", HighlightKind::Variable),
];

thread_local! {
    /// Per-thread cache: configuring a grammar's queries is not free, so the
    /// configuration is built once and reused for every highlight pass.
    static RUST_CONFIG: RefCell<Option<HighlightConfiguration>> = const { RefCell::new(None) };
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

/// Highlight `text` as `language`, returning merged, non-overlapping spans in
/// source order. Returns no spans for files past [`MAX_HIGHLIGHT_BYTES`].
pub fn compute(language: Language, text: &str) -> Vec<HighlightSpan> {
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return Vec::new();
    }
    with_config(language, |config| {
        HIGHLIGHTER.with(|cell| {
            let mut highlighter = cell.borrow_mut();
            match highlighter.highlight(config, text.as_bytes(), None, |_| None) {
                Ok(events) => collect_spans(events),
                Err(_) => Vec::new(),
            }
        })
    })
}

fn with_config<R>(language: Language, f: impl FnOnce(&HighlightConfiguration) -> R) -> R {
    match language {
        Language::Rust => RUST_CONFIG.with(|cell| {
            let mut slot = cell.borrow_mut();
            f(slot.get_or_insert_with(build_rust_config))
        }),
    }
}

fn build_rust_config() -> HighlightConfiguration {
    // The query ships inside the grammar crate; a parse failure would be a
    // build-time bug in that dependency, not a runtime condition.
    let mut config = HighlightConfiguration::new(
        tree_sitter_rust::LANGUAGE.into(),
        "rust",
        tree_sitter_rust::HIGHLIGHTS_QUERY,
        "",
        "",
    )
    .expect("tree-sitter-rust ships a valid highlight query");
    let names: Vec<&str> = CAPTURES.iter().map(|(name, _)| *name).collect();
    config.configure(&names);
    config
}

/// Flatten tree-sitter's nested highlight events into merged spans. The
/// innermost (most recently started) highlight wins for each source chunk.
fn collect_spans(
    events: impl Iterator<Item = Result<HighlightEvent, tree_sitter_highlight::Error>>,
) -> Vec<HighlightSpan> {
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut stack: Vec<Option<HighlightKind>> = Vec::new();
    for event in events {
        let Ok(event) = event else {
            break;
        };
        match event {
            HighlightEvent::HighlightStart(highlight) => {
                stack.push(CAPTURES.get(highlight.0).map(|(_, kind)| *kind));
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if start >= end {
                    continue;
                }
                let Some(Some(kind)) = stack.last().copied() else {
                    continue;
                };
                match spans.last_mut() {
                    Some(prev) if prev.kind == kind && prev.range.end == start => {
                        prev.range.end = end;
                    }
                    _ => spans.push(HighlightSpan {
                        range: start..end,
                        kind,
                    }),
                }
            }
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds_at<'a>(spans: &'a [HighlightSpan], text: &'a str, needle: &str) -> Vec<HighlightKind> {
        let at = text.find(needle).expect("needle present");
        spans
            .iter()
            .filter(|span| span.range.start <= at && at < span.range.end)
            .map(|span| span.kind)
            .collect()
    }

    #[test]
    fn language_resolves_from_extension_and_path() {
        assert_eq!(Language::from_extension("rs"), Some(Language::Rust));
        assert_eq!(Language::from_extension("RS"), Some(Language::Rust));
        assert_eq!(Language::from_extension("txt"), None);
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")),
            Some(Language::Rust)
        );
        assert_eq!(Language::from_path(Path::new("README")), None);
    }

    #[test]
    fn rust_keywords_and_comments_are_highlighted() {
        let text = "// note\nfn main() {}\n";
        let spans = compute(Language::Rust, text);
        assert_eq!(kinds_at(&spans, text, "// note"), [HighlightKind::Comment]);
        assert_eq!(kinds_at(&spans, text, "fn"), [HighlightKind::Keyword]);
        assert_eq!(kinds_at(&spans, text, "main"), [HighlightKind::Function]);
    }

    #[test]
    fn rust_strings_are_highlighted() {
        let text = "fn f() { let s = \"hello\"; }";
        let spans = compute(Language::Rust, text);
        assert_eq!(kinds_at(&spans, text, "\"hello\""), [HighlightKind::String]);
    }

    #[test]
    fn spans_are_sorted_and_non_overlapping() {
        let text = "struct Point { x: i32 }\nfn area() -> i32 { 0 }\n";
        let spans = compute(Language::Rust, text);
        assert!(!spans.is_empty());
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "overlapping spans: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn large_files_skip_highlighting() {
        let text = "fn f() {}\n".repeat(MAX_HIGHLIGHT_BYTES / 10 + 1);
        assert!(text.len() > MAX_HIGHLIGHT_BYTES);
        assert!(compute(Language::Rust, &text).is_empty());
    }
}
