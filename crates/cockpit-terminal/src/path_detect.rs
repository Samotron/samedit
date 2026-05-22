//! Detection of file-path references in terminal output.
//!
//! Recognises references of the form `path`, `path:line`, and `path:line:col`
//! (spec §17). Detection is intentionally a heuristic: a token only counts as
//! a path when it contains a `/` separator or ends in a file extension, which
//! keeps bare words (`note:`), bare numbers (`42`), and URLs
//! (`https://host:443`) from matching too eagerly.
//!
//! Not yet handled (tracked for a later pass):
//! - Windows drive-letter paths (`C:\src\main.rs`)
//! - the Python-traceback form (`File "path", line N`)

use std::ops::Range;

/// A single file reference found in a chunk of terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    /// The file path exactly as it appeared in the text.
    pub path: String,
    /// 1-based line number, when the reference included one.
    pub line: Option<u32>,
    /// 1-based column number, when the reference included one.
    pub column: Option<u32>,
    /// Byte range of the whole reference within the input text.
    pub span: Range<usize>,
}

impl PathMatch {
    /// The canonical `path[:line[:col]]` text for this reference. Feeding it
    /// back through [`detect_paths`] recovers the same path, line, and column.
    pub fn reference(&self) -> String {
        let mut text = self.path.clone();
        if let Some(line) = self.line {
            text.push(':');
            text.push_str(&line.to_string());
            if let Some(column) = self.column {
                text.push(':');
                text.push_str(&column.to_string());
            }
        }
        text
    }
}

/// Characters that terminate a path token.
fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | '"' | '\'' | '`' | ',' | ';' | '|'
        )
}

/// Scan `text` and return every file reference it contains, in order.
pub fn detect_paths(text: &str) -> Vec<PathMatch> {
    let mut matches = Vec::new();
    let mut token_start: Option<usize> = None;

    for (i, c) in text.char_indices() {
        if is_delimiter(c) {
            if let Some(start) = token_start.take()
                && let Some(m) = parse_token(&text[start..i], start)
            {
                matches.push(m);
            }
        } else if token_start.is_none() {
            token_start = Some(i);
        }
    }
    if let Some(start) = token_start
        && let Some(m) = parse_token(&text[start..], start)
    {
        matches.push(m);
    }
    matches
}

/// Parse one whitespace-delimited token at byte `offset` into a [`PathMatch`].
fn parse_token(raw: &str, offset: usize) -> Option<PathMatch> {
    // Drop trailing sentence punctuation that is unlikely to be part of a path.
    let trimmed = raw.trim_end_matches(['.', ',', ':', ';']);
    if trimmed.is_empty() {
        return None;
    }

    // Peel up to two trailing all-digit `:`-segments as line and column.
    let segs: Vec<&str> = trimmed.split(':').collect();
    let mut end = segs.len();
    let mut nums: Vec<u32> = Vec::new();
    while end > 1 && nums.len() < 2 {
        let Ok(n) = segs[end - 1].parse::<u32>() else {
            break;
        };
        nums.push(n);
        end -= 1;
    }
    nums.reverse(); // peeled right-to-left; restore [line, col] order.

    let path = segs[..end].join(":");
    if !looks_like_path(&path) {
        return None;
    }

    Some(PathMatch {
        path,
        line: nums.first().copied(),
        column: nums.get(1).copied(),
        span: offset..offset + trimmed.len(),
    })
}

/// Heuristic: does this string look like a file path worth surfacing?
fn looks_like_path(path: &str) -> bool {
    if path.is_empty() || path.contains("://") {
        return false;
    }
    path.contains('/') || has_extension(path)
}

/// True if the final path component ends in a `.ext` of ASCII alphanumerics.
fn has_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => {
            name[dot + 1..].chars().all(|c| c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collapse matches to `(path, line, col)` triples for terse assertions.
    fn paths(text: &str) -> Vec<(String, Option<u32>, Option<u32>)> {
        detect_paths(text)
            .into_iter()
            .map(|m| (m.path, m.line, m.column))
            .collect()
    }

    #[test]
    fn detects_path_line_col() {
        assert_eq!(
            paths("src/main.rs:42:13"),
            vec![("src/main.rs".to_string(), Some(42), Some(13))]
        );
    }

    #[test]
    fn detects_path_line() {
        assert_eq!(
            paths("tests/test_api.py:120"),
            vec![("tests/test_api.py".to_string(), Some(120), None)]
        );
    }

    #[test]
    fn detects_path_only() {
        assert_eq!(
            paths("see app/foo.py here"),
            vec![("app/foo.py".to_string(), None, None)]
        );
    }

    #[test]
    fn detects_inside_rust_error_arrow() {
        assert_eq!(
            paths("  --> src/main.rs:42:13"),
            vec![("src/main.rs".to_string(), Some(42), Some(13))]
        );
    }

    #[test]
    fn detects_multiple_in_one_line() {
        assert_eq!(
            paths("edit src/a.rs:1 and src/b.rs:2:3"),
            vec![
                ("src/a.rs".to_string(), Some(1), None),
                ("src/b.rs".to_string(), Some(2), Some(3)),
            ]
        );
    }

    #[test]
    fn ignores_bare_words_and_numbers() {
        assert!(detect_paths("note: something happened at line 42").is_empty());
    }

    #[test]
    fn ignores_urls() {
        assert!(detect_paths("listening on https://localhost:8080").is_empty());
    }

    #[test]
    fn trims_trailing_sentence_punctuation() {
        assert_eq!(
            paths("the error is in src/main.rs:10."),
            vec![("src/main.rs".to_string(), Some(10), None)]
        );
    }

    #[test]
    fn span_covers_the_reference() {
        let text = "at src/main.rs:7:2 done";
        let m = &detect_paths(text)[0];
        assert_eq!(&text[m.span.clone()], "src/main.rs:7:2");
    }

    #[test]
    fn detects_extensionless_path_with_slash() {
        assert_eq!(
            paths("./scripts/build"),
            vec![("./scripts/build".to_string(), None, None)]
        );
    }

    #[test]
    fn handles_empty_input() {
        assert!(detect_paths("").is_empty());
    }

    #[test]
    fn reference_round_trips_through_detection() {
        for text in ["src/main.rs", "src/main.rs:42", "src/main.rs:42:13"] {
            let m = &detect_paths(text)[0];
            assert_eq!(m.reference(), text);
        }
    }
}
