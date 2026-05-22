//! Reversible edit history for [`Buffer`](crate::Buffer).

use std::ops::Range;

use crate::Buffer;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    start: usize,
    deleted: String,
    inserted: String,
}

impl Edit {
    fn inserted_range(&self) -> Range<usize> {
        self.start..self.start + self.inserted.len()
    }
}

/// Undo/redo stack for buffer replacements.
#[derive(Debug, Clone, Default)]
pub struct History {
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl History {
    /// Create an empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// True when there is an edit available to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// True when there is an edit available to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Apply a replacement and record it as one undoable edit.
    pub fn replace(&mut self, buffer: &mut Buffer, range: Range<usize>, text: &str) {
        let len = buffer.len_bytes();
        let start = range.start.min(len);
        let end = range.end.min(len).max(start);
        let deleted = buffer.replace(start..end, text);
        if deleted.is_empty() && text.is_empty() {
            return;
        }
        self.undo.push(Edit {
            start,
            deleted,
            inserted: text.to_string(),
        });
        self.redo.clear();
    }

    /// Apply an insertion and record it as one undoable edit.
    pub fn insert(&mut self, buffer: &mut Buffer, byte: usize, text: &str) {
        self.replace(buffer, byte..byte, text);
    }

    /// Apply a deletion and record it as one undoable edit.
    pub fn delete(&mut self, buffer: &mut Buffer, range: Range<usize>) {
        self.replace(buffer, range, "");
    }

    /// Undo the latest edit. Returns `true` when an edit was applied.
    pub fn undo(&mut self, buffer: &mut Buffer) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        buffer.replace(edit.inserted_range(), &edit.deleted);
        self.redo.push(edit);
        true
    }

    /// Redo the latest undone edit. Returns `true` when an edit was applied.
    pub fn redo(&mut self, buffer: &mut Buffer) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        let deleted_end = edit.start + edit.deleted.len();
        buffer.replace(edit.start..deleted_end, &edit.inserted);
        self.undo.push(edit);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_undo_redo() {
        let mut buffer = Buffer::from("hello");
        let mut history = History::new();
        history.insert(&mut buffer, 5, " world");
        assert_eq!(buffer.text(), "hello world");
        assert!(history.undo(&mut buffer));
        assert_eq!(buffer.text(), "hello");
        assert!(history.redo(&mut buffer));
        assert_eq!(buffer.text(), "hello world");
    }

    #[test]
    fn deletion_undo_redo() {
        let mut buffer = Buffer::from("hello world");
        let mut history = History::new();
        history.delete(&mut buffer, 5..11);
        assert_eq!(buffer.text(), "hello");
        history.undo(&mut buffer);
        assert_eq!(buffer.text(), "hello world");
        history.redo(&mut buffer);
        assert_eq!(buffer.text(), "hello");
    }

    #[test]
    fn replacement_undo_redo() {
        let mut buffer = Buffer::from("hello world");
        let mut history = History::new();
        history.replace(&mut buffer, 6..11, "there");
        assert_eq!(buffer.text(), "hello there");
        history.undo(&mut buffer);
        assert_eq!(buffer.text(), "hello world");
        history.redo(&mut buffer);
        assert_eq!(buffer.text(), "hello there");
    }

    #[test]
    fn new_edit_clears_redo_stack() {
        let mut buffer = Buffer::from("abc");
        let mut history = History::new();
        history.insert(&mut buffer, 3, "d");
        history.undo(&mut buffer);
        assert!(history.can_redo());
        history.insert(&mut buffer, 0, ">");
        assert!(!history.can_redo());
    }
}
