//! Cursor positioning over a [`Buffer`](crate::Buffer).
//!
//! The cursor stores a byte offset. Movement helpers clamp to valid UTF-8
//! character boundaries by delegating to the buffer.

use crate::Buffer;

/// A single insertion point in a text buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    byte: usize,
}

impl Cursor {
    /// Create a cursor at byte offset 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cursor at `byte`, clamped to `buffer`.
    pub fn at(buffer: &Buffer, byte: usize) -> Self {
        let mut cursor = Self::new();
        cursor.set_byte(buffer, byte);
        cursor
    }

    /// Current byte offset.
    pub fn byte(self) -> usize {
        self.byte
    }

    /// Current `(line, column)` position. Both values are 0-based and columns
    /// are byte offsets within the line.
    pub fn line_col(self, buffer: &Buffer) -> (usize, usize) {
        buffer.byte_to_line_col(self.byte)
    }

    /// Set the cursor to a byte offset, clamped to the buffer length and
    /// snapped to a valid character boundary.
    pub fn set_byte(&mut self, buffer: &Buffer, byte: usize) {
        let byte = byte.min(buffer.len_bytes());
        let previous = buffer.prev_char_boundary(byte);
        let next = buffer.next_char_boundary(previous);
        self.byte = if previous == byte || byte == buffer.len_bytes() {
            byte
        } else {
            next.min(buffer.len_bytes())
        };
    }

    /// Move one character left.
    pub fn move_left(&mut self, buffer: &Buffer) {
        self.byte = buffer.prev_char_boundary(self.byte);
    }

    /// Move one character right.
    pub fn move_right(&mut self, buffer: &Buffer) {
        self.byte = buffer.next_char_boundary(self.byte);
    }

    /// Move one line up, preserving the current byte column where possible.
    pub fn move_up(&mut self, buffer: &Buffer) {
        let (line, col) = self.line_col(buffer);
        if line > 0 {
            self.byte = line_content_byte(buffer, line - 1, col);
        }
    }

    /// Move one line down, preserving the current byte column where possible.
    pub fn move_down(&mut self, buffer: &Buffer) {
        let (line, col) = self.line_col(buffer);
        if line + 1 < buffer.len_lines() {
            self.byte = line_content_byte(buffer, line + 1, col);
        }
    }

    /// Move to the start of the current line.
    pub fn move_to_line_start(&mut self, buffer: &Buffer) {
        let (line, _) = self.line_col(buffer);
        self.byte = buffer.line_to_byte(line);
    }

    /// Move to the end of the current line, before a trailing newline.
    pub fn move_to_line_end(&mut self, buffer: &Buffer) {
        let (line, _) = self.line_col(buffer);
        self.byte = buffer.line_end_byte(line);
    }
}

fn line_content_byte(buffer: &Buffer, line: usize, col: usize) -> usize {
    let start = buffer.line_to_byte(line);
    let end = buffer.line_end_byte(line);
    (start + col).min(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_to_buffer() {
        let buffer = Buffer::from("abc");
        assert_eq!(Cursor::at(&buffer, 99).byte(), 3);
    }

    #[test]
    fn moves_horizontally_by_character() {
        let buffer = Buffer::from("aéb");
        let mut cursor = Cursor::new();
        cursor.move_right(&buffer);
        assert_eq!(cursor.byte(), 1);
        cursor.move_right(&buffer);
        assert_eq!(cursor.byte(), 3);
        cursor.move_left(&buffer);
        assert_eq!(cursor.byte(), 1);
    }

    #[test]
    fn moves_vertically_and_clamps_column() {
        let buffer = Buffer::from("abcd\nxy\nz");
        let mut cursor = Cursor::at(&buffer, 3);
        cursor.move_down(&buffer);
        assert_eq!(cursor.line_col(&buffer), (1, 2));
        cursor.move_down(&buffer);
        assert_eq!(cursor.line_col(&buffer), (2, 1));
    }

    #[test]
    fn moves_to_line_bounds() {
        let buffer = Buffer::from("ab\ncde\n");
        let mut cursor = Cursor::at(&buffer, 4);
        cursor.move_to_line_start(&buffer);
        assert_eq!(cursor.byte(), 3);
        cursor.move_to_line_end(&buffer);
        assert_eq!(cursor.byte(), 6);
    }
}
