//! Locate the nearest test target from the cursor (spec §16 / §17 v0.3).
//!
//! "Run Nearest" needs a name to hand to the test runner; this module walks
//! backwards from the cursor through the buffer and reports the first function
//! declaration it finds. Pure, language-aware, and headless-testable.
//!
//! Currently implemented for Rust (`fn <name>(...)`). Returns `None` for
//! unknown languages or when no function is in scope before the cursor.

use crate::buffer::Buffer;
use crate::highlight::Language;

/// Find the name of the function declaration nearest to (and at or before) the
/// cursor. Returns `None` when no function is found or the language is not
/// supported yet.
pub fn nearest_test_name(
    buffer: &Buffer,
    cursor_byte: usize,
    language: Option<Language>,
) -> Option<String> {
    if language != Some(Language::Rust) {
        return None;
    }
    let text = buffer.text();
    let cursor_byte = cursor_byte.min(text.len());
    // Include the rest of the cursor's line so a cursor inside the `fn` line
    // still matches that fn.
    let line_end = text[cursor_byte..]
        .find('\n')
        .map(|offset| cursor_byte + offset)
        .unwrap_or(text.len());

    text[..line_end].lines().rev().find_map(parse_rust_fn_name)
}

fn parse_rust_fn_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = strip_fn_modifiers(trimmed);
    let name_part = rest.strip_prefix("fn ")?;
    let end = name_part
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(name_part.len());
    if end == 0 {
        return None;
    }
    Some(name_part[..end].to_string())
}

fn strip_fn_modifiers(line: &str) -> &str {
    let mut current = line;
    loop {
        let next = current
            .strip_prefix("pub(crate) ")
            .or_else(|| {
                current
                    .strip_prefix("pub(super) ")
                    .or_else(|| current.strip_prefix("pub "))
            })
            .or_else(|| current.strip_prefix("async "))
            .or_else(|| current.strip_prefix("unsafe "))
            .or_else(|| current.strip_prefix("const "));
        match next {
            Some(rest) => current = rest,
            None => return current,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> Buffer {
        let mut buffer = Buffer::new();
        buffer.insert(0, text);
        buffer
    }

    const SOURCE: &str = r#"#[cfg(test)]
mod tests {
    #[test]
    fn test_alpha() {
        assert_eq!(1, 1);
    }

    #[test]
    pub fn test_beta() {
        assert!(true);
    }

    async fn helper() -> u32 {
        7
    }
}
"#;

    fn byte_offset_of(text: &str, needle: &str) -> usize {
        text.find(needle).expect("needle present")
    }

    #[test]
    fn returns_none_for_non_rust_language() {
        let buffer = buffer("fn test_x() {}");
        assert_eq!(nearest_test_name(&buffer, 0, None), None);
    }

    #[test]
    fn returns_none_when_buffer_has_no_fn_before_cursor() {
        let buffer = buffer("// just a comment\nfn after() {}\n");
        // Cursor inside the comment, before any `fn`.
        let cursor = byte_offset_of("// just a comment\nfn after() {}\n", "just");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            None
        );
    }

    #[test]
    fn finds_the_function_containing_the_cursor() {
        let buffer = buffer(SOURCE);
        let cursor = byte_offset_of(SOURCE, "assert_eq!(1, 1)");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("test_alpha".to_string()),
        );
    }

    #[test]
    fn finds_the_nearest_preceding_function_when_cursor_is_between_them() {
        let buffer = buffer(SOURCE);
        // Cursor on the blank line between test_alpha and test_beta.
        let cursor = byte_offset_of(SOURCE, "    #[test]\n    pub fn test_beta");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("test_alpha".to_string()),
        );
    }

    #[test]
    fn strips_pub_and_async_modifiers() {
        let buffer = buffer(SOURCE);
        let cursor = byte_offset_of(SOURCE, "assert!(true)");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("test_beta".to_string()),
        );

        let cursor = byte_offset_of(SOURCE, "        7");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("helper".to_string()),
        );
    }

    #[test]
    fn finds_fn_on_the_cursor_line_itself() {
        let buffer = buffer("fn solo() {}\n");
        let cursor = byte_offset_of("fn solo() {}\n", "solo");
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("solo".to_string()),
        );
    }

    #[test]
    fn ignores_lines_that_merely_mention_fn() {
        let buffer = buffer("// note: fn fake is not a real fn\nfn real() {}\n");
        let cursor = buffer.len_bytes();
        assert_eq!(
            nearest_test_name(&buffer, cursor, Some(Language::Rust)),
            Some("real".to_string()),
        );
    }
}
