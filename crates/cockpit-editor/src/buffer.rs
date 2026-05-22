//! The text buffer — a thin, byte-addressed wrapper over a `ropey` rope.
//!
//! All public offsets are **byte** offsets into UTF-8 text. Ropey works in
//! `char` indices internally; conversions happen here so the rest of the
//! editor speaks a single coordinate system (spec §15).

use std::ops::Range;

use ropey::Rope;

/// An editable text buffer.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    rope: Rope,
    /// Bumped on every mutation so callers can cache derived data (e.g. syntax
    /// highlights) and recompute only when the text actually changed.
    revision: u64,
}

impl Buffer {
    /// A monotonically increasing counter, incremented on every mutation.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

impl Buffer {
    /// Create an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// The buffer's full contents as a `String`.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Total length of the buffer in bytes.
    pub fn len_bytes(&self) -> usize {
        self.rope.len_bytes()
    }

    /// Number of lines. Always at least 1, even for an empty buffer.
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// True when the buffer holds no text.
    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Replace the byte `range` with `text`, returning the text that was
    /// removed. Out-of-range or inverted ranges are clamped to the buffer.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> String {
        self.revision = self.revision.wrapping_add(1);
        let len = self.len_bytes();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);

        let removed = self.slice(start..end);
        let start_char = self.rope.byte_to_char(start);
        let end_char = self.rope.byte_to_char(end);
        if end_char > start_char {
            self.rope.remove(start_char..end_char);
        }
        if !text.is_empty() {
            self.rope.insert(start_char, text);
        }
        removed
    }

    /// Insert `text` at the given byte offset.
    pub fn insert(&mut self, byte: usize, text: &str) {
        self.replace(byte..byte, text);
    }

    /// Delete the given byte `range`, returning the removed text.
    pub fn delete(&mut self, range: Range<usize>) -> String {
        self.replace(range, "")
    }

    /// The text within the given byte `range`, clamped to the buffer.
    pub fn slice(&self, range: Range<usize>) -> String {
        let len = self.len_bytes();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        let start_char = self.rope.byte_to_char(start);
        let end_char = self.rope.byte_to_char(end);
        self.rope.slice(start_char..end_char).to_string()
    }

    /// Convert a byte offset to a `(line, column)` pair. The line is 0-based;
    /// the column is the byte offset *within* that line.
    pub fn byte_to_line_col(&self, byte: usize) -> (usize, usize) {
        let byte = byte.min(self.len_bytes());
        let line = self.rope.byte_to_line(byte);
        let line_start = self.rope.line_to_byte(line);
        (line, byte - line_start)
    }

    /// Convert a `(line, column)` pair back to a byte offset. Both components
    /// are clamped so the result is always a valid offset.
    pub fn line_col_to_byte(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        let line_start = self.rope.line_to_byte(line);
        let line_limit = if line + 1 < self.len_lines() {
            self.rope.line_to_byte(line + 1)
        } else {
            self.len_bytes()
        };
        (line_start + col).min(line_limit)
    }

    /// Byte offset of the start of `line`.
    pub fn line_to_byte(&self, line: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        self.rope.line_to_byte(line)
    }

    /// Byte offset of the end of `line`'s content, *before* any trailing
    /// newline.
    pub fn line_end_byte(&self, line: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        let start = self.rope.line_to_byte(line);
        let slice = self.rope.line(line);
        let mut end = start + slice.len_bytes();
        let chars = slice.len_chars();
        if chars > 0 && slice.char(chars - 1) == '\n' {
            end -= 1;
        }
        end
    }

    /// Byte offset of the first char boundary strictly before `byte`.
    pub fn prev_char_boundary(&self, byte: usize) -> usize {
        if byte == 0 {
            return 0;
        }
        let byte = byte.min(self.len_bytes());
        let ch = self.rope.byte_to_char(byte);
        let ch_start = self.rope.char_to_byte(ch);
        if ch_start < byte {
            // `byte` fell in the middle of a char; snap to its start.
            ch_start
        } else {
            self.rope.char_to_byte(ch.saturating_sub(1))
        }
    }

    /// Byte offset of the first char boundary strictly after `byte`.
    pub fn next_char_boundary(&self, byte: usize) -> usize {
        let len = self.len_bytes();
        if byte >= len {
            return len;
        }
        let ch = self.rope.byte_to_char(byte);
        let next = (ch + 1).min(self.rope.len_chars());
        self.rope.char_to_byte(next)
    }
}

impl From<&str> for Buffer {
    fn from(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            revision: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_and_text_round_trip() {
        assert_eq!(Buffer::from("hello\nworld").text(), "hello\nworld");
    }

    #[test]
    fn empty_buffer() {
        let b = Buffer::new();
        assert!(b.is_empty());
        assert_eq!(b.len_bytes(), 0);
        assert_eq!(b.len_lines(), 1);
    }

    #[test]
    fn insert_in_middle() {
        let mut b = Buffer::from("helloworld");
        b.insert(5, " ");
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn delete_range() {
        let mut b = Buffer::from("hello world");
        let removed = b.delete(5..11);
        assert_eq!(removed, " world");
        assert_eq!(b.text(), "hello");
    }

    #[test]
    fn replace_range() {
        let mut b = Buffer::from("hello world");
        let removed = b.replace(6..11, "there");
        assert_eq!(removed, "world");
        assert_eq!(b.text(), "hello there");
    }

    #[test]
    fn replace_clamps_out_of_range() {
        let mut b = Buffer::from("abc");
        b.replace(10..20, "!");
        assert_eq!(b.text(), "abc!");
    }

    #[test]
    fn line_counts() {
        assert_eq!(Buffer::from("").len_lines(), 1);
        assert_eq!(Buffer::from("a").len_lines(), 1);
        assert_eq!(Buffer::from("a\nb").len_lines(), 2);
        assert_eq!(Buffer::from("a\n").len_lines(), 2);
    }

    #[test]
    fn byte_to_line_col_mapping() {
        let b = Buffer::from("ab\ncde\nf");
        assert_eq!(b.byte_to_line_col(0), (0, 0));
        assert_eq!(b.byte_to_line_col(2), (0, 2)); // the '\n'
        assert_eq!(b.byte_to_line_col(3), (1, 0)); // 'c'
        assert_eq!(b.byte_to_line_col(7), (2, 0)); // 'f'
    }

    #[test]
    fn line_col_to_byte_mapping() {
        let b = Buffer::from("ab\ncde\nf");
        assert_eq!(b.line_col_to_byte(0, 0), 0);
        assert_eq!(b.line_col_to_byte(1, 0), 3);
        assert_eq!(b.line_col_to_byte(2, 0), 7);
    }

    #[test]
    fn line_end_byte_excludes_newline() {
        let b = Buffer::from("ab\ncde\nf");
        assert_eq!(b.line_end_byte(0), 2); // before '\n'
        assert_eq!(b.line_end_byte(1), 6);
        assert_eq!(b.line_end_byte(2), 8); // last line, no newline
    }

    #[test]
    fn handles_utf8() {
        let mut b = Buffer::from("héllo");
        assert_eq!(b.len_bytes(), 6); // 'é' is two bytes
        b.insert(0, "→");
        assert!(b.text().starts_with("→h"));
    }
}
