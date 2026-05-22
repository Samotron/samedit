//! Golden tests for the Vim state machine (spec §18.5 / M1.19).
//!
//! Each case feeds a key sequence into a fresh [`Vim`] and snapshots the full
//! trace: starting mode, every (key → emitted actions) step, and the final
//! mode. The exact `(buffer, cursor, registers)` checks of spec §18.5 will be
//! layered on once the action applier (M1.14 editor view) is wired up.

use std::fmt::Write;

use cockpit_editor::vim::{Key, Vim};

fn snapshot(case: &str, keys: &[Key]) -> String {
    let mut vim = Vim::new();
    let mut out = String::new();
    writeln!(&mut out, "case: {case}").unwrap();
    writeln!(&mut out, "start_mode: {:?}", vim.mode()).unwrap();
    writeln!(&mut out, "steps:").unwrap();
    for key in keys {
        let actions = vim.step(*key);
        writeln!(&mut out, "  - key: {key:?}").unwrap();
        if actions.is_empty() {
            writeln!(&mut out, "    actions: []").unwrap();
        } else {
            writeln!(&mut out, "    actions:").unwrap();
            for action in &actions {
                writeln!(&mut out, "      - {action:?}").unwrap();
            }
        }
    }
    writeln!(&mut out, "final_mode: {:?}", vim.mode()).unwrap();
    out
}

fn chars(s: &str) -> Vec<Key> {
    s.chars().map(Key::Char).collect()
}

#[test]
fn golden_normal_motions() {
    insta::assert_snapshot!(snapshot("normal_motions", &chars("hjklwbe0^$G")));
}

#[test]
fn golden_gg_and_dd() {
    insta::assert_snapshot!(snapshot("gg_dd", &chars("ggdd")));
}

#[test]
fn golden_insert_then_escape() {
    let keys = vec![
        Key::Char('i'),
        Key::Char('h'),
        Key::Char('i'),
        Key::Enter,
        Key::Char('!'),
        Key::Escape,
    ];
    insta::assert_snapshot!(snapshot("insert_then_escape", &keys));
}

#[test]
fn golden_open_line_below() {
    insta::assert_snapshot!(snapshot("open_line_below", &chars("o")));
}

#[test]
fn golden_command_write_quit() {
    let keys = vec![Key::Char(':'), Key::Char('w'), Key::Char('q'), Key::Enter];
    insta::assert_snapshot!(snapshot("command_wq", &keys));
}

#[test]
fn golden_search() {
    let keys = vec![
        Key::Char('/'),
        Key::Char('f'),
        Key::Char('o'),
        Key::Char('o'),
        Key::Enter,
    ];
    insta::assert_snapshot!(snapshot("search_foo", &keys));
}

#[test]
fn golden_search_repeat() {
    let keys = vec![
        Key::Char('/'),
        Key::Char('f'),
        Key::Char('o'),
        Key::Char('o'),
        Key::Enter,
        Key::Char('n'),
        Key::Char('N'),
    ];
    insta::assert_snapshot!(snapshot("search_repeat", &keys));
}

#[test]
fn golden_undo_redo_delete_paste() {
    let keys = vec![
        Key::Char('x'),
        Key::Char('p'),
        Key::Char('u'),
        Key::Ctrl('r'),
    ];
    insta::assert_snapshot!(snapshot("undo_redo_delete_paste", &keys));
}

#[test]
fn golden_count_motion() {
    insta::assert_snapshot!(snapshot("count_motion", &chars("3w")));
}

#[test]
fn golden_count_jump() {
    insta::assert_snapshot!(snapshot("count_jump", &chars("12G")));
}

#[test]
fn golden_operator_motion() {
    insta::assert_snapshot!(snapshot("operator_motion", &chars("d2w")));
}

#[test]
fn golden_change_word() {
    insta::assert_snapshot!(snapshot("change_word", &chars("cw")));
}

#[test]
fn golden_visual_delete() {
    insta::assert_snapshot!(snapshot("visual_delete", &chars("vlld")));
}

#[test]
fn golden_visual_line_yank() {
    insta::assert_snapshot!(snapshot("visual_line_yank", &chars("Vjy")));
}

#[test]
fn golden_replace_char() {
    insta::assert_snapshot!(snapshot("replace_char", &chars("rx")));
}

#[test]
fn golden_replace_mode() {
    let keys = vec![Key::Char('R'), Key::Char('a'), Key::Char('b'), Key::Escape];
    insta::assert_snapshot!(snapshot("replace_mode", &keys));
}
