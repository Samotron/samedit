//! The editable document — [`Buffer`] + [`Cursor`] + [`Vim`] + [`History`].
//!
//! [`Editor`] is the aggregate that the application drives: it feeds keys into
//! the Vim state machine and applies the resulting [`Action`]s to the buffer.
//! It is fully headless — no rendering, no filesystem — so the whole
//! edit/undo/motion surface is unit-testable (spec §18.2, §18.5).

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::search;
use crate::undo::History;
use crate::vim::{Action, AppCommand, Key, LineDirection, Mode, Vim};

/// An app-level request raised by a key (the `:w`/`:q`/`:wq` family).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorSignal {
    /// The key produced nothing the application must act on.
    None,
    /// `:w` — write the buffer to its file.
    Save,
    /// `:q` — close the document.
    Quit,
    /// `:wq` / `:x` — write, then close.
    SaveQuit,
}

/// An editable document driven by the Vim state machine.
#[derive(Debug)]
pub struct Editor {
    buffer: Buffer,
    cursor: Cursor,
    vim: Vim,
    history: History,
    /// Linewise yank/delete register, fed by `dd`/`yy` and read by `p`.
    register: String,
    dirty: bool,
}

impl Editor {
    /// Create an editor over `text`, cursor at the start, in Normal mode.
    pub fn new(text: &str) -> Self {
        Self {
            buffer: Buffer::from(text),
            cursor: Cursor::new(),
            vim: Vim::new(),
            history: History::new(),
            register: String::new(),
            dirty: false,
        }
    }

    /// The text buffer.
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// The cursor.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The Vim state machine (mode, pending command line, search query).
    pub fn vim(&self) -> &Vim {
        &self.vim
    }

    /// Current editor mode.
    pub fn mode(&self) -> Mode {
        self.vim.mode()
    }

    /// True when the buffer has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Full buffer contents.
    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Mark the document clean — call after a successful write.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Feed one key, applying every action it produces. Returns the app-level
    /// signal raised by the key, if any.
    pub fn handle_key(&mut self, key: Key) -> EditorSignal {
        let mut signal = EditorSignal::None;
        for action in self.vim.step(key) {
            if let Some(raised) = self.apply(action) {
                signal = raised;
            }
        }
        signal
    }

    /// Apply one Vim action to the buffer/cursor. Returns a signal for the
    /// `AppCommand` actions the application must handle.
    fn apply(&mut self, action: Action) -> Option<EditorSignal> {
        match action {
            Action::MoveLeft => self.cursor.move_left(&self.buffer),
            Action::MoveRight => self.cursor.move_right(&self.buffer),
            Action::MoveLine(LineDirection::Up) => self.cursor.move_up(&self.buffer),
            Action::MoveLine(LineDirection::Down) => self.cursor.move_down(&self.buffer),
            Action::MoveWordForward => {
                let target = next_word_start(&self.buffer.text(), self.cursor.byte());
                self.cursor.set_byte(&self.buffer, target);
            }
            Action::MoveWordBackward => {
                let target = prev_word_start(&self.buffer.text(), self.cursor.byte());
                self.cursor.set_byte(&self.buffer, target);
            }
            Action::MoveWordEnd => {
                let target = word_end(&self.buffer.text(), self.cursor.byte());
                self.cursor.set_byte(&self.buffer, target);
            }
            Action::MoveLineStart => self.cursor.move_to_line_start(&self.buffer),
            Action::MoveFirstNonWhitespace => self.move_first_non_whitespace(),
            Action::MoveLineEnd => self.cursor.move_to_line_end(&self.buffer),
            Action::MoveFileStart => self.cursor.set_byte(&self.buffer, 0),
            Action::MoveFileEnd => {
                let last = self.buffer.len_lines().saturating_sub(1);
                let byte = self.buffer.line_to_byte(last);
                self.cursor.set_byte(&self.buffer, byte);
            }
            Action::EnterMode(_) => {}
            Action::InsertChar(c) => self.insert_char(c),
            Action::DeleteChar => self.delete_char(),
            Action::DeleteLine => self.delete_line(),
            Action::YankLine => self.yank_line(),
            Action::PasteAfter => self.paste_after(),
            Action::Undo => {
                if self.history.undo(&mut self.buffer) {
                    self.dirty = true;
                    self.reclamp_cursor();
                }
            }
            Action::Redo => {
                if self.history.redo(&mut self.buffer) {
                    self.dirty = true;
                    self.reclamp_cursor();
                }
            }
            Action::Search(query) => {
                if let Some(found) = search::find_next(&self.buffer, &query, self.cursor.byte() + 1)
                    .or_else(|| search::find_next(&self.buffer, &query, 0))
                {
                    self.cursor.set_byte(&self.buffer, found.start);
                }
            }
            Action::AppCommand(command) => {
                return Some(match command {
                    AppCommand::Save => EditorSignal::Save,
                    AppCommand::Quit => EditorSignal::Quit,
                    AppCommand::SaveQuit => EditorSignal::SaveQuit,
                });
            }
        }
        None
    }

    fn move_first_non_whitespace(&mut self) {
        let (line, _) = self.cursor.line_col(&self.buffer);
        let start = self.buffer.line_to_byte(line);
        let end = self.buffer.line_end_byte(line);
        let text = self.buffer.slice(start..end);
        let offset = text
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.cursor.set_byte(&self.buffer, start + offset);
    }

    fn insert_char(&mut self, c: char) {
        let at = self.cursor.byte();
        let mut encoded = [0u8; 4];
        let text = c.encode_utf8(&mut encoded);
        self.history.insert(&mut self.buffer, at, text);
        self.cursor.set_byte(&self.buffer, at + text.len());
        self.dirty = true;
    }

    fn delete_char(&mut self) {
        let at = self.cursor.byte();
        let end = self.buffer.next_char_boundary(at);
        if end > at {
            self.history.delete(&mut self.buffer, at..end);
            self.cursor.set_byte(&self.buffer, at);
            self.dirty = true;
        }
    }

    /// Byte range `[start, end)` of the current line, newline included.
    fn current_line_range(&self) -> (usize, usize) {
        let (line, _) = self.cursor.line_col(&self.buffer);
        let start = self.buffer.line_to_byte(line);
        let end = if line + 1 < self.buffer.len_lines() {
            self.buffer.line_to_byte(line + 1)
        } else {
            self.buffer.len_bytes()
        };
        (start, end)
    }

    fn delete_line(&mut self) {
        let (start, end) = self.current_line_range();
        if end <= start {
            return;
        }
        self.register = linewise(self.buffer.slice(start..end));
        self.history.delete(&mut self.buffer, start..end);
        self.cursor.set_byte(&self.buffer, start);
        self.dirty = true;
    }

    fn yank_line(&mut self) {
        let (start, end) = self.current_line_range();
        self.register = linewise(self.buffer.slice(start..end));
    }

    fn paste_after(&mut self) {
        if self.register.is_empty() {
            return;
        }
        let (line, _) = self.cursor.line_col(&self.buffer);
        if line + 1 < self.buffer.len_lines() {
            let at = self.buffer.line_to_byte(line + 1);
            let text = self.register.clone();
            self.history.insert(&mut self.buffer, at, &text);
            self.cursor.set_byte(&self.buffer, at);
        } else {
            // Pasting below the final line: add the separating newline first.
            let at = self.buffer.len_bytes();
            let text = format!("\n{}", self.register.trim_end_matches('\n'));
            self.history.insert(&mut self.buffer, at, &text);
            self.cursor.set_byte(&self.buffer, at + 1);
        }
        self.dirty = true;
    }

    fn reclamp_cursor(&mut self) {
        let byte = self.cursor.byte();
        self.cursor.set_byte(&self.buffer, byte);
    }
}

/// Ensure a yanked/deleted span is linewise (terminated by a newline).
fn linewise(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Byte offset of the next word start after `pos` (Vim `w`). A word is a run
/// of non-whitespace characters.
fn next_word_start(text: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = match chars.iter().position(|(byte, _)| *byte >= pos) {
        Some(i) => i,
        None => return text.len(),
    };
    if !chars[i].1.is_whitespace() {
        while i < chars.len() && !chars[i].1.is_whitespace() {
            i += 1;
        }
    }
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1;
    }
    chars.get(i).map(|(byte, _)| *byte).unwrap_or(text.len())
}

/// Byte offset of the previous word start before `pos` (Vim `b`).
fn prev_word_start(text: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = match chars.iter().rposition(|(byte, _)| *byte < pos) {
        Some(i) => i,
        None => return 0,
    };
    while i > 0 && chars[i].1.is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].1.is_whitespace() {
        i -= 1;
    }
    chars.get(i).map(|(byte, _)| *byte).unwrap_or(0)
}

/// Byte offset of the end of the next word at or after `pos` (Vim `e`).
fn word_end(text: &str, pos: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return 0;
    }
    let mut i = match chars.iter().position(|(byte, _)| *byte > pos) {
        Some(i) => i,
        None => return chars.last().map(|(byte, _)| *byte).unwrap_or(0),
    };
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1;
    }
    while i + 1 < chars.len() && !chars[i + 1].1.is_whitespace() {
        i += 1;
    }
    chars
        .get(i)
        .map(|(byte, _)| *byte)
        .unwrap_or_else(|| text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &str) -> impl Iterator<Item = Key> + '_ {
        input.chars().map(Key::Char)
    }

    fn feed(editor: &mut Editor, input: &str) {
        for key in keys(input) {
            editor.handle_key(key);
        }
    }

    #[test]
    fn insert_mode_typing_edits_the_buffer() {
        let mut editor = Editor::new("");
        editor.handle_key(Key::Char('i'));
        feed(&mut editor, "hello");
        editor.handle_key(Key::Escape);

        assert_eq!(editor.text(), "hello");
        assert_eq!(editor.mode(), Mode::Normal);
        assert!(editor.is_dirty());
    }

    #[test]
    fn hjkl_motions_move_the_cursor() {
        let mut editor = Editor::new("abc\ndef");
        feed(&mut editor, "ll");
        assert_eq!(editor.cursor().byte(), 2);
        feed(&mut editor, "j");
        assert_eq!(editor.cursor().line_col(editor.buffer()), (1, 2));
        feed(&mut editor, "h");
        assert_eq!(editor.cursor().line_col(editor.buffer()), (1, 1));
    }

    #[test]
    fn x_deletes_the_character_under_the_cursor() {
        let mut editor = Editor::new("abc");
        editor.handle_key(Key::Char('x'));
        assert_eq!(editor.text(), "bc");
    }

    #[test]
    fn dd_deletes_a_line_and_p_pastes_it_below() {
        let mut editor = Editor::new("one\ntwo\nthree");
        feed(&mut editor, "dd");
        assert_eq!(editor.text(), "two\nthree");
        feed(&mut editor, "p");
        assert_eq!(editor.text(), "two\none\nthree");
    }

    #[test]
    fn yy_then_p_duplicates_a_line() {
        let mut editor = Editor::new("solo");
        feed(&mut editor, "yyp");
        assert_eq!(editor.text(), "solo\nsolo");
    }

    #[test]
    fn undo_and_redo_round_trip_an_edit() {
        let mut editor = Editor::new("abc");
        editor.handle_key(Key::Char('x'));
        assert_eq!(editor.text(), "bc");
        editor.handle_key(Key::Char('u'));
        assert_eq!(editor.text(), "abc");
        editor.handle_key(Key::Ctrl('r'));
        assert_eq!(editor.text(), "bc");
    }

    #[test]
    fn word_motions_step_across_whitespace() {
        let mut editor = Editor::new("alpha beta gamma");
        editor.handle_key(Key::Char('w'));
        assert_eq!(editor.cursor().byte(), 6); // start of "beta"
        editor.handle_key(Key::Char('e'));
        assert_eq!(editor.cursor().byte(), 9); // 'a' ending "beta"
        editor.handle_key(Key::Char('b'));
        assert_eq!(editor.cursor().byte(), 6); // back to "beta"
    }

    #[test]
    fn gg_and_capital_g_jump_between_file_ends() {
        let mut editor = Editor::new("one\ntwo\nthree");
        editor.handle_key(Key::Char('G'));
        assert_eq!(editor.cursor().line_col(editor.buffer()).0, 2);
        feed(&mut editor, "gg");
        assert_eq!(editor.cursor().byte(), 0);
    }

    #[test]
    fn write_command_raises_a_save_signal() {
        let mut editor = Editor::new("data");
        let signal = [Key::Char(':'), Key::Char('w'), Key::Enter]
            .into_iter()
            .map(|key| editor.handle_key(key))
            .last()
            .unwrap();
        assert_eq!(signal, EditorSignal::Save);
    }

    #[test]
    fn search_jumps_to_the_next_match() {
        let mut editor = Editor::new("foo bar foo");
        for key in [
            Key::Char('/'),
            Key::Char('b'),
            Key::Char('a'),
            Key::Char('r'),
            Key::Enter,
        ] {
            editor.handle_key(key);
        }
        assert_eq!(editor.cursor().byte(), 4);
    }

    #[test]
    fn mark_saved_clears_the_dirty_flag() {
        let mut editor = Editor::new("");
        editor.handle_key(Key::Char('i'));
        editor.handle_key(Key::Char('z'));
        assert!(editor.is_dirty());
        editor.mark_saved();
        assert!(!editor.is_dirty());
    }
}
