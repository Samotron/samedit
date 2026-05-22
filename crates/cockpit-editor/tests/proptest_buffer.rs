//! Property-based invariants for the rope buffer and undo history
//! (spec §18.4 / M2.8).
//!
//! Strings are restricted to ASCII (excluding the NUL byte) so every offset
//! is automatically a char boundary; the conversion edge-cases at multi-byte
//! boundaries are covered by the unit tests in [`buffer`](cockpit_editor::buffer).

use cockpit_editor::{Buffer, History};
use proptest::collection::vec;
use proptest::prelude::*;

/// Printable ASCII plus '\n', no NUL.
fn ascii_text() -> impl Strategy<Value = String> {
    "[\x20-\x7E\n]{0,40}".prop_map(|s| s.to_string())
}

#[derive(Debug, Clone)]
enum Edit {
    Insert {
        byte: usize,
        text: String,
    },
    Delete {
        start: usize,
        end: usize,
    },
    Replace {
        start: usize,
        end: usize,
        text: String,
    },
}

fn arb_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0..=64usize, ascii_text()).prop_map(|(byte, text)| Edit::Insert { byte, text }),
        (0..=64usize, 0..=64usize).prop_map(|(a, b)| Edit::Delete {
            start: a.min(b),
            end: a.max(b)
        }),
        (0..=64usize, 0..=64usize, ascii_text()).prop_map(|(a, b, text)| Edit::Replace {
            start: a.min(b),
            end: a.max(b),
            text
        }),
    ]
}

fn clamp(range: (usize, usize), len: usize) -> (usize, usize) {
    let start = range.0.min(len);
    let end = range.1.min(len).max(start);
    (start, end)
}

fn apply_to_reference(reference: &mut String, edit: &Edit) {
    let len = reference.len();
    match edit {
        Edit::Insert { byte, text } => {
            let at = (*byte).min(len);
            reference.insert_str(at, text);
        }
        Edit::Delete { start, end } => {
            let (s, e) = clamp((*start, *end), len);
            reference.replace_range(s..e, "");
        }
        Edit::Replace { start, end, text } => {
            let (s, e) = clamp((*start, *end), len);
            reference.replace_range(s..e, text);
        }
    }
}

fn apply_to_buffer(buffer: &mut Buffer, edit: &Edit) {
    match edit {
        Edit::Insert { byte, text } => {
            buffer.insert(*byte, text);
        }
        Edit::Delete { start, end } => {
            buffer.delete(*start..*end);
        }
        Edit::Replace { start, end, text } => {
            buffer.replace(*start..*end, text);
        }
    }
}

fn apply_to_history(buffer: &mut Buffer, history: &mut History, edit: &Edit) {
    match edit {
        Edit::Insert { byte, text } => history.insert(buffer, *byte, text),
        Edit::Delete { start, end } => history.delete(buffer, *start..*end),
        Edit::Replace { start, end, text } => history.replace(buffer, *start..*end, text),
    }
}

proptest! {
    /// The rope-backed buffer must agree byte-for-byte with a plain `String`
    /// after any sequence of clamped edits (spec §18.4).
    #[test]
    fn buffer_matches_reference_string(
        initial in ascii_text(),
        edits in vec(arb_edit(), 0..16),
    ) {
        let mut buffer = Buffer::from(initial.as_str());
        let mut reference = initial.clone();
        for edit in &edits {
            apply_to_buffer(&mut buffer, edit);
            apply_to_reference(&mut reference, edit);
            prop_assert_eq!(buffer.text(), reference.clone());
            prop_assert_eq!(buffer.len_bytes(), reference.len());
        }
    }

    /// Inserting `text` at `byte`, then deleting exactly the inserted range,
    /// returns the original buffer.
    #[test]
    fn insert_then_delete_round_trips(
        initial in ascii_text(),
        text in ascii_text(),
        byte in 0..=128usize,
    ) {
        let mut buffer = Buffer::from(initial.as_str());
        let len = buffer.len_bytes();
        let at = byte.min(len);

        buffer.insert(at, &text);
        buffer.delete(at..at + text.len());

        prop_assert_eq!(buffer.text(), initial);
    }

    /// Undoing every recorded edit must restore the original buffer
    /// regardless of edit order or kind.
    #[test]
    fn undo_all_restores_initial(
        initial in ascii_text(),
        edits in vec(arb_edit(), 0..16),
    ) {
        let mut buffer = Buffer::from(initial.as_str());
        let mut history = History::new();
        for edit in &edits {
            apply_to_history(&mut buffer, &mut history, edit);
        }
        while history.can_undo() {
            prop_assert!(history.undo(&mut buffer));
        }
        prop_assert_eq!(buffer.text(), initial);
    }

    /// After undoing every edit and redoing every edit, the buffer must match
    /// the state produced by simply applying every edit once.
    #[test]
    fn redo_after_undo_matches_forward_application(
        initial in ascii_text(),
        edits in vec(arb_edit(), 0..16),
    ) {
        let mut a = Buffer::from(initial.as_str());
        let mut history = History::new();
        for edit in &edits {
            apply_to_history(&mut a, &mut history, edit);
        }
        let forward = a.text();

        while history.can_undo() {
            history.undo(&mut a);
        }
        prop_assert_eq!(a.text(), initial.clone());

        while history.can_redo() {
            prop_assert!(history.redo(&mut a));
        }
        prop_assert_eq!(a.text(), forward);
    }

    /// For every valid byte offset, converting to `(line, col)` and back
    /// returns the original byte offset (spec §18.4 offset round-trip).
    #[test]
    fn byte_to_line_col_round_trips(
        text in ascii_text(),
        offset in 0usize..256,
    ) {
        let buffer = Buffer::from(text.as_str());
        let byte = offset.min(buffer.len_bytes());
        let (line, col) = buffer.byte_to_line_col(byte);
        prop_assert_eq!(buffer.line_col_to_byte(line, col), byte);
    }

    /// Loading text into the rope and reading it back returns it unchanged for
    /// arbitrary UTF-8 — the pure analogue of the spec §18.4 save/load round
    /// trip, and the only buffer property exercised on non-ASCII input.
    #[test]
    fn buffer_round_trips_arbitrary_text(text in "\\PC{0,80}") {
        prop_assert_eq!(Buffer::from(text.as_str()).text(), text);
    }
}
