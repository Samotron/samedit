//! `termwiz`-backed terminal parser adapter.
//!
//! `termwiz` provides VT/ANSI parsing. This adapter applies the parsed actions
//! to Cockpit's UI-facing [`ScreenGrid`](crate::engine::ScreenGrid).

use termwiz::escape::{
    Action, ControlCode, OneBased,
    csi::{CSI, Cursor as CsiCursor, Edit, EraseInDisplay, EraseInLine},
    parser::Parser,
};

use crate::engine::{ScreenGrid, TerminalEngine};

/// Terminal engine backed by `termwiz` escape parsing.
pub struct TermwizEngine {
    parser: Parser,
    grid: ScreenGrid,
}

impl TermwizEngine {
    /// Create a new parser-backed engine.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            parser: Parser::new(),
            grid: ScreenGrid::new(width, height),
        }
    }

    fn apply_action(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.grid.put_char(ch),
            Action::PrintString(text) => {
                for ch in text.chars() {
                    self.grid.put_char(ch);
                }
            }
            Action::Control(control) => self.apply_control(control),
            Action::CSI(csi) => self.apply_csi(csi),
            _ => {}
        }
    }

    fn apply_control(&mut self, control: ControlCode) {
        match control {
            ControlCode::Backspace => self.grid.backspace(),
            ControlCode::HorizontalTab => {
                let spaces = 4 - (self.grid.cursor().col % 4);
                for _ in 0..spaces {
                    self.grid.put_char(' ');
                }
            }
            ControlCode::LineFeed | ControlCode::VerticalTab => self.grid.newline(),
            ControlCode::FormFeed => self.grid.clear(),
            ControlCode::CarriageReturn => self.grid.carriage_return(),
            _ => {}
        }
    }

    fn apply_csi(&mut self, csi: CSI) {
        match csi {
            CSI::Cursor(cursor) => self.apply_cursor(cursor),
            CSI::Edit(edit) => self.apply_edit(edit),
            _ => {}
        }
    }

    fn apply_cursor(&mut self, cursor: CsiCursor) {
        match cursor {
            CsiCursor::Left(n) | CsiCursor::CharacterPositionBackward(n) => {
                self.grid.move_cursor(0, -(n as isize));
            }
            CsiCursor::Right(n) | CsiCursor::CharacterPositionForward(n) => {
                self.grid.move_cursor(0, n as isize);
            }
            CsiCursor::Up(n) | CsiCursor::LinePositionBackward(n) => {
                self.grid.move_cursor(-(n as isize), 0);
            }
            CsiCursor::Down(n) | CsiCursor::LinePositionForward(n) => {
                self.grid.move_cursor(n as isize, 0);
            }
            CsiCursor::NextLine(n) => {
                self.grid.move_cursor(n as isize, 0);
                self.grid.carriage_return();
            }
            CsiCursor::PrecedingLine(n) => {
                self.grid.move_cursor(-(n as isize), 0);
                self.grid.carriage_return();
            }
            CsiCursor::Position { line, col }
            | CsiCursor::CharacterAndLinePosition { line, col } => {
                self.grid
                    .set_cursor(one_based_to_zero(line), one_based_to_zero(col));
            }
            CsiCursor::CharacterAbsolute(col) | CsiCursor::CharacterPositionAbsolute(col) => {
                self.grid
                    .set_cursor(self.grid.cursor().row, one_based_to_zero(col));
            }
            CsiCursor::LinePositionAbsolute(line) => {
                self.grid
                    .set_cursor(line.saturating_sub(1) as usize, self.grid.cursor().col);
            }
            _ => {}
        }
    }

    fn apply_edit(&mut self, edit: Edit) {
        match edit {
            Edit::EraseCharacter(n) => self.grid.erase_chars(n as usize),
            Edit::EraseInLine(EraseInLine::EraseToEndOfLine) => self.grid.erase_line_to_end(),
            Edit::EraseInLine(EraseInLine::EraseToStartOfLine) => self.grid.erase_line_to_start(),
            Edit::EraseInLine(EraseInLine::EraseLine) => self.grid.erase_line(),
            Edit::EraseInDisplay(EraseInDisplay::EraseToEndOfDisplay) => {
                self.grid.erase_display_to_end();
            }
            Edit::EraseInDisplay(EraseInDisplay::EraseToStartOfDisplay) => {
                self.grid.erase_display_to_start();
            }
            Edit::EraseInDisplay(EraseInDisplay::EraseDisplay) => self.grid.erase_display(),
            Edit::ScrollUp(n) => {
                for _ in 0..n {
                    self.grid.scroll_up();
                }
            }
            Edit::ScrollDown(n) => {
                for _ in 0..n {
                    self.grid.scroll_down();
                }
            }
            _ => {}
        }
    }
}

impl TerminalEngine for TermwizEngine {
    fn feed(&mut self, bytes: &[u8]) {
        let actions = self.parser.parse_as_vec(bytes);
        for action in actions {
            self.apply_action(action);
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.grid.resize(width, height);
    }

    fn grid(&self) -> &ScreenGrid {
        &self.grid
    }
}

fn one_based_to_zero(value: OneBased) -> usize {
    value.as_zero_based() as usize
}

#[cfg(test)]
mod tests {
    use crate::engine::{Cursor, TerminalEngine};

    use super::*;

    fn rows(engine: &TermwizEngine) -> Vec<String> {
        (0..engine.grid().height())
            .map(|row| engine.grid().row_text(row).unwrap())
            .collect()
    }

    #[test]
    fn parses_plain_text_and_control_codes() {
        let mut engine = TermwizEngine::new(6, 3);
        engine.feed(b"abc\rZ\nnext");

        assert_eq!(rows(&engine), vec!["Zbc   ", "next  ", "      "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 4 });
    }

    #[test]
    fn applies_cursor_positioning() {
        let mut engine = TermwizEngine::new(8, 3);
        engine.feed(b"hello\nworld\x1b[1;2H!");

        assert_eq!(rows(&engine), vec!["h!llo   ", "world   ", "        "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn applies_erase_display() {
        let mut engine = TermwizEngine::new(5, 2);
        engine.feed(b"hello\nworld\x1b[2J");

        assert_eq!(rows(&engine), vec!["     ", "     "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 5 });
    }

    #[test]
    fn applies_erase_line_to_end() {
        let mut engine = TermwizEngine::new(6, 2);
        engine.feed(b"abcdef\x1b[1;3H\x1b[K");

        assert_eq!(rows(&engine), vec!["ab    ", "      "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 0, col: 2 });
    }

    #[test]
    fn resize_preserves_grid_content() {
        let mut engine = TermwizEngine::new(5, 2);
        engine.feed(b"hello\nworld");
        engine.resize(3, 3);

        assert_eq!(rows(&engine), vec!["hel", "wor", "   "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 2 });
    }
}
