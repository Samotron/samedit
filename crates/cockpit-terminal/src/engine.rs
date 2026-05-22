//! Terminal engine contract and headless screen grid model.
//!
//! The production adapter will feed `termwiz` output into this model. Keeping
//! the grid contract independent gives the UI and tests a stable surface while
//! PTY and parser work remains isolated.

/// One terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ' }
    }
}

/// Cursor position in terminal grid coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A fixed-size terminal screen grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenGrid {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    cursor: Cursor,
    wrap_pending: bool,
}

impl ScreenGrid {
    /// Create an empty grid.
    pub fn new(width: usize, height: usize) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        Self {
            width,
            height,
            cells: vec![Cell::default(); width * height],
            cursor: Cursor::default(),
            wrap_pending: false,
        }
    }

    /// Grid width in cells.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Grid height in cells.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Cursor position.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Cell at `row`, `col`.
    pub fn cell(&self, row: usize, col: usize) -> Option<Cell> {
        (row < self.height && col < self.width).then(|| self.cells[self.index(row, col)])
    }

    /// Text for one row, preserving spaces.
    pub fn row_text(&self, row: usize) -> Option<String> {
        if row >= self.height {
            return None;
        }
        Some(
            (0..self.width)
                .map(|col| self.cells[self.index(row, col)].ch)
                .collect(),
        )
    }

    /// Resize the grid, preserving the visible top-left content and clamping
    /// the cursor into bounds.
    pub fn resize(&mut self, width: usize, height: usize) {
        let width = width.max(1);
        let height = height.max(1);
        let mut next = vec![Cell::default(); width * height];
        let copy_height = self.height.min(height);
        let copy_width = self.width.min(width);

        for row in 0..copy_height {
            for col in 0..copy_width {
                next[row * width + col] = self.cells[self.index(row, col)];
            }
        }

        self.width = width;
        self.height = height;
        self.cells = next;
        self.cursor.row = self.cursor.row.min(self.height - 1);
        self.cursor.col = self.cursor.col.min(self.width - 1);
        self.wrap_pending = false;
    }

    pub(crate) fn put_char(&mut self, ch: char) {
        if self.wrap_pending {
            self.newline();
        }
        let col = self.cursor.col.min(self.width - 1);
        let index = self.index(self.cursor.row, col);
        self.cells[index] = Cell { ch };
        self.cursor.col += 1;
        if self.cursor.col >= self.width {
            self.wrap_pending = true;
        }
    }

    pub(crate) fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.wrap_pending = false;
    }

    pub(crate) fn newline(&mut self) {
        self.cursor.col = 0;
        self.wrap_pending = false;
        if self.cursor.row + 1 >= self.height {
            self.scroll_up();
        } else {
            self.cursor.row += 1;
        }
    }

    pub(crate) fn backspace(&mut self) {
        self.wrap_pending = false;
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }

    pub(crate) fn clear(&mut self) {
        self.cells.fill(Cell::default());
        self.cursor = Cursor::default();
        self.wrap_pending = false;
    }

    pub(crate) fn erase_display(&mut self) {
        self.cells.fill(Cell::default());
        self.wrap_pending = false;
    }

    pub(crate) fn scroll_up(&mut self) {
        for row in 1..self.height {
            for col in 0..self.width {
                let from = self.index(row, col);
                let to = self.index(row - 1, col);
                self.cells[to] = self.cells[from];
            }
        }
        let last = self.height - 1;
        for col in 0..self.width {
            let index = self.index(last, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn scroll_down(&mut self) {
        for row in (0..self.height.saturating_sub(1)).rev() {
            for col in 0..self.width {
                let from = self.index(row, col);
                let to = self.index(row + 1, col);
                self.cells[to] = self.cells[from];
            }
        }
        for col in 0..self.width {
            let index = self.index(0, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn move_cursor(&mut self, row_delta: isize, col_delta: isize) {
        self.wrap_pending = false;
        self.cursor.row = self
            .cursor
            .row
            .saturating_add_signed(row_delta)
            .min(self.height - 1);
        self.cursor.col = self
            .cursor
            .col
            .saturating_add_signed(col_delta)
            .min(self.width - 1);
    }

    pub(crate) fn set_cursor(&mut self, row: usize, col: usize) {
        self.wrap_pending = false;
        self.cursor.row = row.min(self.height - 1);
        self.cursor.col = col.min(self.width - 1);
    }

    pub(crate) fn erase_chars(&mut self, count: usize) {
        self.wrap_pending = false;
        let end = self.cursor.col.saturating_add(count).min(self.width);
        for col in self.cursor.col..end {
            let index = self.index(self.cursor.row, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn erase_line_to_end(&mut self) {
        self.wrap_pending = false;
        for col in self.cursor.col..self.width {
            let index = self.index(self.cursor.row, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn erase_line_to_start(&mut self) {
        self.wrap_pending = false;
        for col in 0..=self.cursor.col.min(self.width - 1) {
            let index = self.index(self.cursor.row, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn erase_line(&mut self) {
        self.wrap_pending = false;
        for col in 0..self.width {
            let index = self.index(self.cursor.row, col);
            self.cells[index] = Cell::default();
        }
    }

    pub(crate) fn erase_display_to_end(&mut self) {
        self.erase_line_to_end();
        for row in self.cursor.row + 1..self.height {
            for col in 0..self.width {
                let index = self.index(row, col);
                self.cells[index] = Cell::default();
            }
        }
    }

    pub(crate) fn erase_display_to_start(&mut self) {
        self.erase_line_to_start();
        for row in 0..self.cursor.row {
            for col in 0..self.width {
                let index = self.index(row, col);
                self.cells[index] = Cell::default();
            }
        }
    }

    fn index(&self, row: usize, col: usize) -> usize {
        row * self.width + col
    }
}

/// Terminal engine abstraction used by UI and PTY integration.
pub trait TerminalEngine {
    /// Feed terminal output bytes into the engine.
    fn feed(&mut self, bytes: &[u8]);
    /// Resize the terminal surface.
    fn resize(&mut self, width: usize, height: usize);
    /// Current screen grid.
    fn grid(&self) -> &ScreenGrid;
}

/// Deterministic headless engine used for unit tests and early wire-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridEngine {
    grid: ScreenGrid,
}

impl GridEngine {
    /// Create a new grid engine.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            grid: ScreenGrid::new(width, height),
        }
    }

    fn feed_char(&mut self, ch: char) {
        match ch {
            '\n' => self.grid.newline(),
            '\r' => self.grid.carriage_return(),
            '\u{8}' => self.grid.backspace(),
            '\u{c}' => self.grid.clear(),
            '\t' => {
                let spaces = 4 - (self.grid.cursor.col % 4);
                for _ in 0..spaces {
                    self.grid.put_char(' ');
                }
            }
            ch if !ch.is_control() => self.grid.put_char(ch),
            _ => {}
        }
    }
}

impl TerminalEngine for GridEngine {
    fn feed(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        for ch in text.chars() {
            self.feed_char(ch);
        }
    }

    fn resize(&mut self, width: usize, height: usize) {
        self.grid.resize(width, height);
    }

    fn grid(&self) -> &ScreenGrid {
        &self.grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(engine: &GridEngine) -> Vec<String> {
        (0..engine.grid().height())
            .map(|row| engine.grid().row_text(row).unwrap())
            .collect()
    }

    #[test]
    fn writes_output_to_grid() {
        let mut engine = GridEngine::new(8, 3);
        engine.feed(b"hello");

        assert_eq!(engine.grid().row_text(0).unwrap(), "hello   ");
        assert_eq!(engine.grid().cursor(), Cursor { row: 0, col: 5 });
    }

    #[test]
    fn handles_newline_and_carriage_return() {
        let mut engine = GridEngine::new(6, 3);
        engine.feed(b"abc\rZ\nnext");

        assert_eq!(rows(&engine), vec!["Zbc   ", "next  ", "      "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 4 });
    }

    #[test]
    fn wraps_at_right_edge() {
        let mut engine = GridEngine::new(4, 3);
        engine.feed(b"abcdef");

        assert_eq!(rows(&engine), vec!["abcd", "ef  ", "    "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 2 });
    }

    #[test]
    fn scrolls_when_output_exceeds_height() {
        let mut engine = GridEngine::new(3, 2);
        engine.feed(b"one\ntwo\ntri");

        assert_eq!(rows(&engine), vec!["two", "tri"]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 3 });
    }

    #[test]
    fn clear_resets_grid_and_cursor() {
        let mut engine = GridEngine::new(5, 2);
        engine.feed(b"hello\x0c");

        assert_eq!(rows(&engine), vec!["     ", "     "]);
        assert_eq!(engine.grid().cursor(), Cursor::default());
    }

    #[test]
    fn resize_preserves_visible_content() {
        let mut engine = GridEngine::new(5, 2);
        engine.feed(b"hello\nworld");
        engine.resize(3, 3);

        assert_eq!(rows(&engine), vec!["hel", "wor", "   "]);
        assert_eq!(engine.grid().cursor(), Cursor { row: 1, col: 2 });
    }
}
