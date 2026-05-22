//! Property-based invariants for the [`Editor`] aggregate (spec §18.4 / M2.8).
//!
//! Where [`proptest_buffer`](super) fuzzes the rope buffer directly, this suite
//! drives the whole Vim state machine: random key sequences are fed into an
//! editor and the cursor, selection, undo history, and syntax highlighter are
//! checked for the invariants that keep the buffer from corrupting.

use cockpit_editor::highlight::compute;
use cockpit_editor::vim::Key;
use cockpit_editor::{Editor, Language};
use proptest::collection::vec;
use proptest::prelude::*;

/// Meaningful Vim keys: motions, operators, mode switches, counts, and a
/// little literal text to type in Insert mode.
///
/// Numeric counts have focused unit/golden coverage. They are intentionally
/// omitted here because random count + paste combinations can create very
/// large edit histories from short key streams, making the property suite
/// nondeterministically slow.
const KEY_CHARS: &str = "hjklwbeg^$GiIaAoOvVRrsSxXdcyDCJpPuq:/ z";

fn arb_key() -> impl Strategy<Value = Key> {
    let chars: Vec<char> = KEY_CHARS.chars().collect();
    prop_oneof![
        10 => prop::sample::select(chars).prop_map(Key::Char),
        3 => Just(Key::Escape),
        1 => Just(Key::Enter),
        1 => Just(Key::Backspace),
        1 => Just(Key::Ctrl('r')),
    ]
}

/// Lowercase ASCII plus newline and two multi-byte characters, so cursor and
/// span invariants are exercised at real UTF-8 boundaries.
fn arb_text() -> impl Strategy<Value = String> {
    "[a-z0-9 éλ\n]{0,30}".prop_map(|s| s.to_string())
}

proptest! {
    /// Feeding any sequence of keys must never panic and must always leave the
    /// cursor and visual selection on valid UTF-8 boundaries inside the buffer
    /// (spec §18.4 "cursor never moves outside valid bounds").
    #[test]
    fn editor_keeps_cursor_and_selection_in_bounds(
        initial in arb_text(),
        keys in vec(arb_key(), 0..32),
    ) {
        let mut editor = Editor::new(&initial);
        for key in keys {
            editor.handle_key(key);

            let text = editor.text();
            let byte = editor.cursor().byte();
            prop_assert!(byte <= text.len());
            prop_assert!(text.is_char_boundary(byte));

            let (line, col) = editor.cursor().line_col(editor.buffer());
            prop_assert_eq!(editor.buffer().line_col_to_byte(line, col), byte);

            if let Some((start, end)) = editor.selection() {
                prop_assert!(start <= end);
                prop_assert!(end <= text.len());
                prop_assert!(text.is_char_boundary(start));
                prop_assert!(text.is_char_boundary(end));
            }
        }
    }

    /// Undoing every recorded change returns the buffer to its initial text,
    /// whatever edits the key sequence performed (spec §18.4 undo invariant).
    #[test]
    fn editor_undo_returns_to_initial_text(
        initial in arb_text(),
        keys in vec(arb_key(), 0..32),
    ) {
        let mut editor = Editor::new(&initial);
        for key in keys {
            editor.handle_key(key);
        }
        // Leave any insert/visual/command mode, then undo every edit. Two
        // escapes handle a pending Visual-mode prefix (`vg`) where the first
        // Escape clears the prefix and the second leaves Visual mode.
        editor.handle_key(Key::Escape);
        editor.handle_key(Key::Escape);
        for _ in 0..128 {
            if !editor.can_undo() {
                break;
            }
            editor.handle_key(Key::Char('u'));
        }
        prop_assert!(!editor.can_undo());
        prop_assert_eq!(editor.text(), initial);
    }

    /// Syntax highlighting any text must yield spans that are sorted,
    /// non-overlapping, and aligned to UTF-8 boundaries — the renderer slices
    /// the buffer at these offsets, so a bad span would panic the UI.
    #[test]
    fn highlight_spans_stay_within_bounds(text in "\\PC{0,200}") {
        let spans = compute(Language::Rust, &text);
        let mut previous_end = 0;
        for span in &spans {
            prop_assert!(span.range.start < span.range.end);
            prop_assert!(span.range.end <= text.len());
            prop_assert!(text.is_char_boundary(span.range.start));
            prop_assert!(text.is_char_boundary(span.range.end));
            prop_assert!(span.range.start >= previous_end);
            previous_end = span.range.end;
        }
    }
}
