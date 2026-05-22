//! The editable document — [`Buffer`] + [`Cursor`] + [`Vim`] + [`History`].
//!
//! [`Editor`] is the aggregate that the application drives: it feeds keys into
//! the Vim state machine and applies the resulting [`Action`]s to the buffer.
//! It is fully headless — no rendering, no filesystem — so the whole
//! edit/undo/motion surface is unit-testable (spec §18.2, §18.5).

use crate::buffer::Buffer;
use crate::cursor::Cursor;
use crate::highlight::{self, HighlightSpan, Language};
use crate::search;
use crate::undo::History;
use crate::vim::{Action, AppCommand, Key, LineDirection, Mode, Motion, Operator, PasteWhere, Vim};

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

/// The active visual selection: a fixed `anchor` byte plus the live cursor.
#[derive(Debug, Clone, Copy)]
struct Selection {
    anchor: usize,
    linewise: bool,
}

/// An editable document driven by the Vim state machine.
#[derive(Debug)]
pub struct Editor {
    buffer: Buffer,
    cursor: Cursor,
    vim: Vim,
    history: History,
    /// Yank/delete register, fed by delete/yank actions and read by paste.
    register: String,
    /// Whether [`Editor::register`] holds whole lines (pastes as new lines).
    register_linewise: bool,
    /// Anchor of the visual selection, set while in a Visual mode.
    selection: Option<Selection>,
    /// Syntax-highlighting language, or `None` to disable highlighting.
    language: Option<Language>,
    /// Cached syntax highlights, refreshed when the buffer changes.
    highlights: Vec<HighlightSpan>,
    /// Buffer revision the cached `highlights` were computed at.
    highlight_revision: u64,
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
            register_linewise: false,
            selection: None,
            language: None,
            highlights: Vec::new(),
            highlight_revision: 0,
            dirty: false,
        }
    }

    /// Set the syntax-highlighting language (`None` disables it) and refresh
    /// the cached highlights immediately.
    pub fn set_language(&mut self, language: Option<Language>) {
        self.language = language;
        self.refresh_highlights();
    }

    /// The active syntax-highlighting language, if any.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Syntax-highlight spans for the current buffer, in source order. Empty
    /// when no language is set or the file is too large (spec §15).
    pub fn highlights(&self) -> &[HighlightSpan] {
        &self.highlights
    }

    /// Move the cursor to a 0-based `line` and `column`, clamped to the buffer.
    /// Used by the terminal→editor bridge to jump to a `path:line:col`.
    pub fn goto(&mut self, line: usize, column: usize) {
        let byte = self.buffer.line_col_to_byte(line, column);
        self.cursor.set_byte(&self.buffer, byte);
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

    /// The active visual selection as an inclusive-resolved `[start, end)` byte
    /// range, or `None` when no Visual mode is active.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.selection_span().map(|(start, end, _)| (start, end))
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
        if self.buffer.revision() != self.highlight_revision {
            self.refresh_highlights();
        }
        signal
    }

    fn refresh_highlights(&mut self) {
        self.highlights = match self.language {
            Some(language) => highlight::compute(language, &self.buffer.text()),
            None => Vec::new(),
        };
        self.highlight_revision = self.buffer.revision();
    }

    /// Apply one Vim action to the buffer/cursor. Returns a signal for the
    /// `AppCommand` actions the application must handle.
    fn apply(&mut self, action: Action) -> Option<EditorSignal> {
        match action {
            Action::Move(motion) => {
                let target = self.motion_target(motion);
                self.cursor.set_byte(&self.buffer, target);
            }
            Action::EnterMode(mode) => self.enter_mode(mode),
            Action::InsertChar(c) => self.insert_char(c),
            Action::ReplaceChar(c) => self.replace_char(c),
            Action::DeleteChar => self.delete_char(),
            Action::DeleteLine(count) => {
                let (start, end) = self.line_block_from_cursor(count);
                self.delete_span(start, end, true);
            }
            Action::YankLine(count) => {
                let (start, end) = self.line_block_from_cursor(count);
                let text = self.buffer.slice(start..end);
                self.set_register(text, true);
            }
            Action::ChangeLine(count) => {
                let (start, end) = self.line_block_from_cursor(count);
                self.change_lines(start, end);
            }
            Action::DeleteToLineEnd | Action::ChangeToLineEnd => {
                let (line, _) = self.cursor.line_col(&self.buffer);
                let start = self.cursor.byte();
                let end = self.buffer.line_end_byte(line);
                self.delete_span(start, end, false);
            }
            Action::JoinLines => self.join_lines(),
            Action::Operate { operator, motion } => self.operate(operator, motion),
            Action::DeleteSelection => {
                if let Some((start, end, linewise)) = self.selection_span() {
                    self.delete_span(start, end, linewise);
                }
            }
            Action::YankSelection => {
                if let Some((start, end, linewise)) = self.selection_span() {
                    let text = self.buffer.slice(start..end);
                    self.set_register(text, linewise);
                    self.cursor.set_byte(&self.buffer, start);
                }
            }
            Action::ChangeSelection => {
                if let Some((start, end, linewise)) = self.selection_span() {
                    if linewise {
                        self.change_lines(start, end);
                    } else {
                        self.delete_span(start, end, false);
                    }
                }
            }
            Action::Paste(PasteWhere::After) => self.paste(false),
            Action::Paste(PasteWhere::Before) => self.paste(true),
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

    /// Resolve a [`Motion`] to the byte offset the cursor would move to.
    fn motion_target(&self, motion: Motion) -> usize {
        let cursor = self.cursor.byte();
        match motion {
            Motion::Left => self.buffer.prev_char_boundary(cursor),
            Motion::Right => self.buffer.next_char_boundary(cursor),
            Motion::Line(LineDirection::Up) => {
                let mut moved = self.cursor;
                moved.move_up(&self.buffer);
                moved.byte()
            }
            Motion::Line(LineDirection::Down) => {
                let mut moved = self.cursor;
                moved.move_down(&self.buffer);
                moved.byte()
            }
            Motion::WordForward => next_word_start(&self.buffer.text(), cursor),
            Motion::WordBackward => prev_word_start(&self.buffer.text(), cursor),
            Motion::WordEnd => word_end(&self.buffer.text(), cursor),
            Motion::LineStart => {
                let (line, _) = self.cursor.line_col(&self.buffer);
                self.buffer.line_to_byte(line)
            }
            Motion::FirstNonWhitespace => self.first_non_whitespace_byte(),
            Motion::LineEnd => {
                let (line, _) = self.cursor.line_col(&self.buffer);
                self.buffer.line_end_byte(line)
            }
            Motion::FileStart => 0,
            Motion::FileEnd => {
                let last = self.buffer.len_lines().saturating_sub(1);
                self.buffer.line_to_byte(last)
            }
            Motion::ToLine(line) => self.buffer.line_to_byte(line),
        }
    }

    fn enter_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Visual => {
                let anchor = self.selection_anchor();
                self.selection = Some(Selection {
                    anchor,
                    linewise: false,
                });
            }
            Mode::VisualLine => {
                let anchor = self.selection_anchor();
                self.selection = Some(Selection {
                    anchor,
                    linewise: true,
                });
            }
            Mode::Normal | Mode::Insert | Mode::Replace | Mode::Command | Mode::Search => {
                self.selection = None;
            }
        }
    }

    /// Anchor for a (re)entered Visual mode: keep the existing one when toggling
    /// `v`↔`V`, otherwise anchor at the cursor.
    fn selection_anchor(&self) -> usize {
        self.selection
            .map(|sel| sel.anchor)
            .unwrap_or_else(|| self.cursor.byte())
    }

    /// The visual selection resolved to a `[start, end)` byte range plus a
    /// linewise flag. Charwise selections include the character under the
    /// cursor; linewise selections cover whole lines.
    fn selection_span(&self) -> Option<(usize, usize, bool)> {
        let sel = self.selection?;
        let cursor = self.cursor.byte();
        let lo = sel.anchor.min(cursor);
        let hi = sel.anchor.max(cursor);
        if sel.linewise {
            let lo_line = self.buffer.byte_to_line_col(lo).0;
            let hi_line = self.buffer.byte_to_line_col(hi).0;
            let (start, end) = self.line_block_bytes(lo_line, hi_line);
            Some((start, end, true))
        } else {
            let end = self.buffer.next_char_boundary(hi);
            Some((lo, end, false))
        }
    }

    fn operate(&mut self, operator: Operator, motion: Motion) {
        let (start, end, linewise) = self.operator_span(motion);
        match operator {
            Operator::Delete => self.delete_span(start, end, linewise),
            Operator::Yank => {
                let text = self.buffer.slice(start..end);
                self.set_register(text, linewise);
                self.cursor.set_byte(&self.buffer, start);
            }
            Operator::Change => {
                if linewise {
                    self.change_lines(start, end);
                } else {
                    self.delete_span(start, end, false);
                }
            }
        }
    }

    /// The `[start, end)` byte range an operator covers for `motion`, plus a
    /// linewise flag (vertical and file-jump motions operate on whole lines).
    fn operator_span(&self, motion: Motion) -> (usize, usize, bool) {
        let cursor = self.cursor.byte();
        let target = self.motion_target(motion);
        let linewise = matches!(
            motion,
            Motion::Line(_) | Motion::FileStart | Motion::FileEnd | Motion::ToLine(_)
        );
        if linewise {
            let cur_line = self.buffer.byte_to_line_col(cursor).0;
            let tgt_line = self.buffer.byte_to_line_col(target).0;
            let (start, end) =
                self.line_block_bytes(cur_line.min(tgt_line), cur_line.max(tgt_line));
            (start, end, true)
        } else {
            let lo = cursor.min(target);
            let mut hi = cursor.max(target);
            // `e` is inclusive of the character it lands on.
            if matches!(motion, Motion::WordEnd) {
                hi = self.buffer.next_char_boundary(hi);
            }
            (lo, hi, false)
        }
    }

    fn first_non_whitespace_byte(&self) -> usize {
        let (line, _) = self.cursor.line_col(&self.buffer);
        let start = self.buffer.line_to_byte(line);
        let end = self.buffer.line_end_byte(line);
        let text = self.buffer.slice(start..end);
        let offset = text
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(0);
        start + offset
    }

    fn insert_char(&mut self, c: char) {
        let at = self.cursor.byte();
        let mut encoded = [0u8; 4];
        let text = c.encode_utf8(&mut encoded);
        self.history.insert(&mut self.buffer, at, text);
        self.cursor.set_byte(&self.buffer, at + text.len());
        self.dirty = true;
    }

    /// Overwrite the character under the cursor with `c` and step right. At the
    /// end of a line (or the buffer) there is nothing to overwrite, so insert.
    fn replace_char(&mut self, c: char) {
        let at = self.cursor.byte();
        let end = self.buffer.next_char_boundary(at);
        let on_newline = end > at && self.buffer.slice(at..end) == "\n";
        let mut encoded = [0u8; 4];
        let text = c.encode_utf8(&mut encoded);
        if at >= self.buffer.len_bytes() || on_newline {
            self.history.insert(&mut self.buffer, at, text);
        } else {
            self.history.replace(&mut self.buffer, at..end, text);
        }
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

    /// Byte range `[start, end)` of `count` whole lines from the cursor line,
    /// each newline included.
    fn line_block_from_cursor(&self, count: usize) -> (usize, usize) {
        let (line, _) = self.cursor.line_col(&self.buffer);
        let last = self.buffer.len_lines().saturating_sub(1);
        let hi = (line + count.saturating_sub(1)).min(last);
        self.line_block_bytes(line, hi)
    }

    /// Byte range `[start, end)` spanning whole lines `lo..=hi`.
    fn line_block_bytes(&self, lo: usize, hi: usize) -> (usize, usize) {
        let start = self.buffer.line_to_byte(lo);
        let end = if hi + 1 < self.buffer.len_lines() {
            self.buffer.line_to_byte(hi + 1)
        } else {
            self.buffer.len_bytes()
        };
        (start, end)
    }

    fn delete_span(&mut self, start: usize, end: usize, linewise: bool) {
        if end <= start {
            return;
        }
        let removed = self.buffer.slice(start..end);
        self.set_register(removed, linewise);
        self.history.delete(&mut self.buffer, start..end);
        self.cursor.set_byte(&self.buffer, start);
        self.dirty = true;
    }

    /// Clear the contents of lines `[start, end)` but leave one empty line in
    /// their place, ready for Insert mode (`cc` / `Vc`).
    fn change_lines(&mut self, start: usize, end: usize) {
        let block = self.buffer.slice(start..end);
        self.set_register(block, true);
        // Drop a single trailing newline so one empty line survives.
        let del_end =
            if end > start && self.buffer.slice(self.buffer.prev_char_boundary(end)..end) == "\n" {
                self.buffer.prev_char_boundary(end)
            } else {
                end
            };
        if del_end > start {
            self.history.delete(&mut self.buffer, start..del_end);
            self.dirty = true;
        }
        self.cursor.set_byte(&self.buffer, start);
    }

    /// Join the cursor line with the one below: drop the newline and the next
    /// line's leading whitespace, separating the two with a single space.
    fn join_lines(&mut self) {
        let (line, _) = self.cursor.line_col(&self.buffer);
        if line + 1 >= self.buffer.len_lines() {
            return;
        }
        let join_at = self.buffer.line_end_byte(line);
        let next_start = self.buffer.line_to_byte(line + 1);
        let next_end = self.buffer.line_end_byte(line + 1);
        let next = self.buffer.slice(next_start..next_end);
        let leading_ws = next.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let separator = if next.trim_start().is_empty() {
            ""
        } else {
            " "
        };
        self.history.replace(
            &mut self.buffer,
            join_at..next_start + leading_ws,
            separator,
        );
        self.cursor.set_byte(&self.buffer, join_at);
        self.dirty = true;
    }

    fn set_register(&mut self, text: String, linewise: bool) {
        self.register = if linewise { as_linewise(text) } else { text };
        self.register_linewise = linewise;
    }

    fn paste(&mut self, before: bool) {
        if self.register.is_empty() {
            return;
        }
        if self.register_linewise {
            self.paste_linewise(before);
        } else {
            self.paste_charwise(before);
        }
    }

    fn paste_linewise(&mut self, before: bool) {
        let text = self.register.clone();
        let (line, _) = self.cursor.line_col(&self.buffer);
        if before {
            let at = self.buffer.line_to_byte(line);
            self.history.insert(&mut self.buffer, at, &text);
            self.cursor.set_byte(&self.buffer, at);
        } else if line + 1 < self.buffer.len_lines() {
            let at = self.buffer.line_to_byte(line + 1);
            self.history.insert(&mut self.buffer, at, &text);
            self.cursor.set_byte(&self.buffer, at);
        } else {
            // Pasting below the final line: add the separating newline first.
            let at = self.buffer.len_bytes();
            let text = format!("\n{}", text.trim_end_matches('\n'));
            self.history.insert(&mut self.buffer, at, &text);
            self.cursor.set_byte(&self.buffer, at + 1);
        }
        self.dirty = true;
    }

    fn paste_charwise(&mut self, before: bool) {
        let text = self.register.clone();
        let at = if before {
            self.cursor.byte()
        } else {
            self.buffer.next_char_boundary(self.cursor.byte())
        };
        self.history.insert(&mut self.buffer, at, &text);
        // Land the cursor on the last pasted character, Vim-style.
        let last = self.buffer.prev_char_boundary(at + text.len());
        self.cursor.set_byte(&self.buffer, last);
        self.dirty = true;
    }

    fn reclamp_cursor(&mut self) {
        let byte = self.cursor.byte();
        self.cursor.set_byte(&self.buffer, byte);
    }
}

/// Ensure a yanked/deleted span is linewise (terminated by a newline).
fn as_linewise(mut text: String) -> String {
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
    fn count_repeats_x() {
        let mut editor = Editor::new("abcdef");
        feed(&mut editor, "3x");
        assert_eq!(editor.text(), "def");
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
    fn count_deletes_multiple_lines() {
        let mut editor = Editor::new("one\ntwo\nthree\nfour");
        feed(&mut editor, "2dd");
        assert_eq!(editor.text(), "three\nfour");
    }

    #[test]
    fn yy_then_p_duplicates_a_line() {
        let mut editor = Editor::new("solo");
        feed(&mut editor, "yyp");
        assert_eq!(editor.text(), "solo\nsolo");
    }

    #[test]
    fn capital_p_pastes_a_line_above() {
        let mut editor = Editor::new("one\ntwo");
        feed(&mut editor, "yyjP");
        assert_eq!(editor.text(), "one\none\ntwo");
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
    fn count_g_jumps_to_a_line() {
        let mut editor = Editor::new("one\ntwo\nthree\nfour");
        feed(&mut editor, "3G");
        assert_eq!(editor.cursor().line_col(editor.buffer()).0, 2);
    }

    #[test]
    fn dw_deletes_a_word() {
        let mut editor = Editor::new("alpha beta gamma");
        feed(&mut editor, "dw");
        assert_eq!(editor.text(), "beta gamma");
    }

    #[test]
    fn de_deletes_to_word_end_inclusive() {
        let mut editor = Editor::new("alpha beta");
        feed(&mut editor, "de");
        assert_eq!(editor.text(), " beta");
    }

    #[test]
    fn cw_changes_a_word_and_enters_insert() {
        let mut editor = Editor::new("alpha beta");
        feed(&mut editor, "cw");
        assert_eq!(editor.mode(), Mode::Insert);
        feed(&mut editor, "ALPHA");
        editor.handle_key(Key::Escape);
        assert_eq!(editor.text(), "ALPHAbeta");
    }

    #[test]
    fn capital_d_deletes_to_end_of_line() {
        let mut editor = Editor::new("hello world");
        feed(&mut editor, "wD");
        assert_eq!(editor.text(), "hello ");
    }

    #[test]
    fn r_replaces_one_character() {
        let mut editor = Editor::new("cat");
        feed(&mut editor, "rb");
        assert_eq!(editor.text(), "bat");
        assert_eq!(editor.cursor().byte(), 0);
        assert_eq!(editor.mode(), Mode::Normal);
    }

    #[test]
    fn replace_mode_overwrites_characters() {
        let mut editor = Editor::new("abcdef");
        editor.handle_key(Key::Char('R'));
        feed(&mut editor, "XY");
        editor.handle_key(Key::Escape);
        assert_eq!(editor.text(), "XYcdef");
    }

    #[test]
    fn capital_j_joins_lines() {
        let mut editor = Editor::new("hello\n  world");
        editor.handle_key(Key::Char('J'));
        assert_eq!(editor.text(), "hello world");
    }

    #[test]
    fn visual_mode_deletes_a_selection() {
        let mut editor = Editor::new("abcdef");
        feed(&mut editor, "vlld");
        assert_eq!(editor.text(), "def");
        assert_eq!(editor.mode(), Mode::Normal);
    }

    #[test]
    fn visual_line_mode_deletes_whole_lines() {
        let mut editor = Editor::new("one\ntwo\nthree");
        feed(&mut editor, "Vjd");
        assert_eq!(editor.text(), "three");
    }

    #[test]
    fn visual_mode_yank_then_paste() {
        let mut editor = Editor::new("abcdef");
        feed(&mut editor, "vly");
        assert_eq!(editor.cursor().byte(), 0);
        feed(&mut editor, "p");
        assert_eq!(editor.text(), "aabbcdef");
    }

    #[test]
    fn escape_leaves_visual_mode() {
        let mut editor = Editor::new("abc");
        feed(&mut editor, "vl");
        assert!(editor.selection().is_some());
        editor.handle_key(Key::Escape);
        assert_eq!(editor.mode(), Mode::Normal);
        assert!(editor.selection().is_none());
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

    #[test]
    fn goto_moves_the_cursor_to_a_line_and_column() {
        let mut editor = Editor::new("alpha\nbravo\ncharlie");
        editor.goto(1, 3);
        assert_eq!(editor.cursor().line_col(editor.buffer()), (1, 3));
        // Out-of-range positions clamp into the buffer.
        editor.goto(99, 99);
        assert_eq!(editor.cursor().line_col(editor.buffer()).0, 2);
    }

    #[test]
    fn setting_a_language_enables_highlighting() {
        let mut editor = Editor::new("fn main() {}");
        assert!(editor.highlights().is_empty());
        editor.set_language(Some(Language::Rust));
        assert!(!editor.highlights().is_empty());
    }

    #[test]
    fn highlights_refresh_after_an_edit() {
        let mut editor = Editor::new("");
        editor.set_language(Some(Language::Rust));
        assert!(editor.highlights().is_empty());
        editor.handle_key(Key::Char('i'));
        for c in "fn f() {}".chars() {
            editor.handle_key(Key::Char(c));
        }
        editor.handle_key(Key::Escape);
        assert!(
            editor
                .highlights()
                .iter()
                .any(|span| span.kind == highlight::HighlightKind::Keyword)
        );
    }
}
