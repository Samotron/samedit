//! Golden tests for the editor — the exact `(buffer, cursor, mode)` contract
//! of spec §18.5 (M1.19).
//!
//! `golden_vim.rs` snapshots the pure FSM's emitted actions; this file
//! snapshots the *result* of applying them: feed a key sequence into an
//! [`Editor`] over a starting buffer and capture the final buffer (with the
//! cursor marked), mode, cursor line:col, and dirty flag.

use std::fmt::Write;

use cockpit_editor::Editor;
use cockpit_editor::vim::Key;

/// Cursor marker inserted into the rendered buffer. Never used in test input.
const CURSOR: char = '‸';

fn render_keys(keys: &[Key]) -> String {
    keys.iter()
        .map(|key| match key {
            Key::Char(c) => c.to_string(),
            Key::Ctrl(c) => format!("^{c}"),
            Key::Enter => "<CR>".to_string(),
            Key::Escape => "<Esc>".to_string(),
            Key::Backspace => "<BS>".to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn snapshot(case: &str, start: &str, keys: &[Key]) -> String {
    let mut editor = Editor::new(start);
    for key in keys {
        editor.handle_key(*key);
    }

    let buffer = editor.buffer();
    let cursor = editor.cursor();
    let (line, col) = cursor.line_col(buffer);
    let text = buffer.text();
    let mark = cursor.byte().min(text.len());
    let marked = format!("{}{CURSOR}{}", &text[..mark], &text[mark..]);

    let mut out = String::new();
    writeln!(&mut out, "case:   {case}").unwrap();
    writeln!(&mut out, "start:  {start:?}").unwrap();
    writeln!(&mut out, "keys:   {}", render_keys(keys)).unwrap();
    writeln!(&mut out, "mode:   {:?}", editor.mode()).unwrap();
    writeln!(&mut out, "cursor: {line}:{col}").unwrap();
    writeln!(&mut out, "dirty:  {}", editor.is_dirty()).unwrap();
    writeln!(&mut out, "buffer:").unwrap();
    for line in marked.split('\n') {
        writeln!(&mut out, "  | {line}").unwrap();
    }
    out
}

fn chars(input: &str) -> Vec<Key> {
    input.chars().map(Key::Char).collect()
}

#[test]
fn golden_normal_motions() {
    insta::assert_snapshot!(snapshot(
        "normal_motions",
        "alpha\nbravo\ncharlie",
        &chars("lljl")
    ));
}

#[test]
fn golden_word_motions() {
    insta::assert_snapshot!(snapshot(
        "word_motions",
        "alpha beta gamma delta",
        &chars("wwe")
    ));
}

#[test]
fn golden_insert_text() {
    let mut keys = vec![Key::Char('i')];
    keys.extend(chars("hello"));
    keys.push(Key::Escape);
    insta::assert_snapshot!(snapshot("insert_text", "", &keys));
}

#[test]
fn golden_append_after_cursor() {
    let mut keys = vec![Key::Char('a')];
    keys.extend(chars("X"));
    keys.push(Key::Escape);
    insta::assert_snapshot!(snapshot("append_after_cursor", "ab", &keys));
}

#[test]
fn golden_open_line_below() {
    let mut keys = vec![Key::Char('o')];
    keys.extend(chars("new line"));
    keys.push(Key::Escape);
    insta::assert_snapshot!(snapshot("open_line_below", "first\nlast", &keys));
}

#[test]
fn golden_delete_char() {
    insta::assert_snapshot!(snapshot("delete_char", "abcdef", &chars("xx")));
}

#[test]
fn golden_delete_line() {
    insta::assert_snapshot!(snapshot("delete_line", "one\ntwo\nthree", &chars("jdd")));
}

#[test]
fn golden_yank_and_paste() {
    insta::assert_snapshot!(snapshot("yank_and_paste", "duplicate", &chars("yyp")));
}

#[test]
fn golden_undo_then_redo() {
    let keys = vec![
        Key::Char('x'),
        Key::Char('x'),
        Key::Char('u'),
        Key::Ctrl('r'),
    ];
    insta::assert_snapshot!(snapshot("undo_then_redo", "abcdef", &keys));
}

#[test]
fn golden_search_jumps_to_match() {
    let mut keys = vec![Key::Char('/')];
    keys.extend(chars("three"));
    keys.push(Key::Enter);
    insta::assert_snapshot!(snapshot("search_jumps_to_match", "one two three", &keys));
}

#[test]
fn golden_go_to_file_ends() {
    insta::assert_snapshot!(snapshot(
        "go_to_file_ends",
        "top\nmiddle\nbottom",
        &chars("Ggg")
    ));
}
