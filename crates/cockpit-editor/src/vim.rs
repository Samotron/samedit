//! Pure Vim-style input state machine.
//!
//! The state machine translates keys into editor/app actions. It does not own
//! a buffer and performs no I/O, which keeps it easy to golden-test and lets the
//! UI dispatch the same actions through the command layer later.

/// Editor mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Command,
    Search,
    Visual,
    VisualLine,
    Replace,
}

/// Keyboard input understood by the Vim state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Escape,
    Backspace,
}

/// Application command emitted from command-line mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    Save,
    Quit,
    SaveQuit,
}

/// Direction for linewise movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDirection {
    Up,
    Down,
}

/// A cursor motion. Resolved against the buffer by the editor — the state
/// machine stays pure and buffer-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Line(LineDirection),
    WordForward,
    WordBackward,
    WordEnd,
    LineStart,
    FirstNonWhitespace,
    LineEnd,
    FileStart,
    FileEnd,
    /// Jump to a specific 0-based line (`{count}G` / `{count}gg`).
    ToLine(usize),
}

/// An operator that consumes a motion or a visual selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

/// Where a paste lands relative to the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteWhere {
    After,
    Before,
}

/// Buffer or app action produced by a key transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Move the cursor along `motion`.
    Move(Motion),
    EnterMode(Mode),
    InsertChar(char),
    /// Overwrite the character under the cursor and step right (`r`, Replace mode).
    ReplaceChar(char),
    /// Delete the character under the cursor (`x`).
    DeleteChar,
    /// Delete `count` whole lines starting at the cursor line (`dd`).
    DeleteLine(usize),
    /// Yank `count` whole lines starting at the cursor line (`yy`).
    YankLine(usize),
    /// Clear `count` lines, leaving one empty line ready to insert (`cc`).
    ChangeLine(usize),
    /// Delete from the cursor to the end of the line (`D`).
    DeleteToLineEnd,
    /// Delete from the cursor to the end of the line, ready to insert (`C`).
    ChangeToLineEnd,
    /// Join the cursor line with the one below it (`J`).
    JoinLines,
    /// Apply `operator` over `motion` (`dw`, `ce`, `y$`, …).
    Operate {
        operator: Operator,
        motion: Motion,
    },
    /// Delete the visual selection (`d` / `x` in Visual mode).
    DeleteSelection,
    /// Yank the visual selection (`y` in Visual mode).
    YankSelection,
    /// Delete the visual selection, ready to insert (`c` in Visual mode).
    ChangeSelection,
    /// Paste the register relative to the cursor (`p` / `P`).
    Paste(PasteWhere),
    Undo,
    Redo,
    Search(String),
    SearchPrevious(String),
    AppCommand(AppCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    /// Saw `g`; `count` is any prefix typed before it.
    G { count: usize },
    /// Saw `r`; waiting for the replacement character. `count` repeats it.
    Replace { count: usize },
    /// Saw an operator; waiting for a motion (or the doubled operator key).
    /// `count` is the prefix typed before the operator.
    Operator { operator: Operator, count: usize },
}

/// Pure Vim input state.
#[derive(Debug, Clone, Default)]
pub struct Vim {
    mode: Mode,
    pending: Option<Pending>,
    /// Numeric count accumulated from digit keys; `0` means "no count".
    count: usize,
    command: String,
    search: String,
    last_search: Option<String>,
}

impl Vim {
    /// Create a new state machine in Normal mode.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current editor mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Text typed so far on the `:` command line (without the leading `:`).
    pub fn command_line(&self) -> &str {
        &self.command
    }

    /// Text typed so far for a `/` search (without the leading `/`).
    pub fn search_query(&self) -> &str {
        &self.search
    }

    /// Feed one key and receive the actions it produced.
    pub fn step(&mut self, key: Key) -> Vec<Action> {
        match self.mode {
            Mode::Normal | Mode::Visual | Mode::VisualLine => self.normal_family(key),
            Mode::Insert => self.insert(key),
            Mode::Replace => self.replace_mode(key),
            Mode::Command => self.command(key),
            Mode::Search => self.search(key),
        }
    }

    fn set_mode(&mut self, mode: Mode) -> Vec<Action> {
        self.mode = mode;
        self.pending = None;
        self.count = 0;
        vec![Action::EnterMode(mode)]
    }

    /// Keys shared by Normal, Visual, and Visual-line modes: count prefixes,
    /// pending multi-key commands, and motions.
    fn normal_family(&mut self, key: Key) -> Vec<Action> {
        // Count prefix: digits accumulate. A leading `0` is the line-start
        // motion, so `0` only counts as a digit once a count is in progress.
        if let Key::Char(c) = key
            && let Some(d) = c.to_digit(10)
            && !(d == 0 && self.count == 0)
        {
            self.count = self.count.saturating_mul(10).saturating_add(d as usize);
            return Vec::new();
        }

        let count = std::mem::take(&mut self.count);

        if let Some(pending) = self.pending.take() {
            return self.complete_pending(pending, key, count);
        }

        if let Some(motion) = motion_key(key) {
            return vec![Action::Move(motion); count.max(1)];
        }

        match key {
            Key::Char('g') => {
                self.pending = Some(Pending::G { count });
                Vec::new()
            }
            Key::Char('G') => vec![Action::Move(jump_motion(count, Motion::FileEnd))],
            Key::Escape => {
                if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                    self.set_mode(Mode::Normal)
                } else {
                    Vec::new()
                }
            }
            _ => match self.mode {
                Mode::Visual | Mode::VisualLine => self.visual_key(key),
                _ => self.normal_key(key, count),
            },
        }
    }

    /// Normal-mode keys that are not plain motions.
    fn normal_key(&mut self, key: Key, count: usize) -> Vec<Action> {
        let n = count.max(1);
        match key {
            Key::Char('i') => self.set_mode(Mode::Insert),
            Key::Char('I') => self.enter_insert_after(Action::Move(Motion::FirstNonWhitespace)),
            Key::Char('a') => self.enter_insert_after(Action::Move(Motion::Right)),
            Key::Char('A') => self.enter_insert_after(Action::Move(Motion::LineEnd)),
            Key::Char('o') => {
                let mut actions = vec![Action::Move(Motion::LineEnd), Action::InsertChar('\n')];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('O') => {
                let mut actions = vec![
                    Action::Move(Motion::LineStart),
                    Action::InsertChar('\n'),
                    Action::Move(Motion::Left),
                ];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('R') => self.set_mode(Mode::Replace),
            Key::Char('v') => self.set_mode(Mode::Visual),
            Key::Char('V') => self.set_mode(Mode::VisualLine),
            Key::Char('x') => vec![Action::DeleteChar; n],
            Key::Char('s') => {
                let mut actions = vec![Action::DeleteChar; n];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('S') => {
                let mut actions = vec![Action::ChangeLine(n)];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('D') => vec![Action::DeleteToLineEnd],
            Key::Char('C') => {
                let mut actions = vec![Action::ChangeToLineEnd];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('J') => vec![Action::JoinLines; n.max(2) - 1],
            Key::Char('r') => {
                self.pending = Some(Pending::Replace { count });
                Vec::new()
            }
            Key::Char('d') => {
                self.pending = Some(Pending::Operator {
                    operator: Operator::Delete,
                    count,
                });
                Vec::new()
            }
            Key::Char('c') => {
                self.pending = Some(Pending::Operator {
                    operator: Operator::Change,
                    count,
                });
                Vec::new()
            }
            Key::Char('y') => {
                self.pending = Some(Pending::Operator {
                    operator: Operator::Yank,
                    count,
                });
                Vec::new()
            }
            Key::Char('u') => vec![Action::Undo; n],
            Key::Ctrl('r') | Key::Ctrl('R') => vec![Action::Redo; n],
            Key::Char('p') => vec![Action::Paste(PasteWhere::After); n],
            Key::Char('P') => vec![Action::Paste(PasteWhere::Before); n],
            Key::Char('n') => self
                .last_search
                .clone()
                .map(|query| vec![Action::Search(query); n])
                .unwrap_or_default(),
            Key::Char('N') => self
                .last_search
                .clone()
                .map(|query| vec![Action::SearchPrevious(query); n])
                .unwrap_or_default(),
            Key::Char(':') => {
                self.command.clear();
                self.set_mode(Mode::Command)
            }
            Key::Char('/') => {
                self.search.clear();
                self.set_mode(Mode::Search)
            }
            _ => Vec::new(),
        }
    }

    /// Visual / Visual-line keys that are not motions: mode toggles and the
    /// selection-consuming operators.
    fn visual_key(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Char('v') => {
                if self.mode == Mode::Visual {
                    self.set_mode(Mode::Normal)
                } else {
                    self.set_mode(Mode::Visual)
                }
            }
            Key::Char('V') => {
                if self.mode == Mode::VisualLine {
                    self.set_mode(Mode::Normal)
                } else {
                    self.set_mode(Mode::VisualLine)
                }
            }
            Key::Char('d') | Key::Char('x') => {
                let mut actions = vec![Action::DeleteSelection];
                actions.extend(self.set_mode(Mode::Normal));
                actions
            }
            Key::Char('y') => {
                let mut actions = vec![Action::YankSelection];
                actions.extend(self.set_mode(Mode::Normal));
                actions
            }
            Key::Char('c') | Key::Char('s') => {
                let mut actions = vec![Action::ChangeSelection];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            _ => Vec::new(),
        }
    }

    fn enter_insert_after(&mut self, setup: Action) -> Vec<Action> {
        let mut actions = vec![setup];
        actions.extend(self.set_mode(Mode::Insert));
        actions
    }

    fn complete_pending(&mut self, pending: Pending, key: Key, count: usize) -> Vec<Action> {
        match pending {
            Pending::G { count: pre } => match key {
                Key::Char('g') => {
                    let count = if pre > 0 { pre } else { count };
                    vec![Action::Move(jump_motion(count, Motion::FileStart))]
                }
                _ => Vec::new(),
            },
            Pending::Replace { count: pre } => match key {
                Key::Char(c) => {
                    let mut actions = vec![Action::ReplaceChar(c); pre.max(1)];
                    actions.push(Action::Move(Motion::Left));
                    actions
                }
                _ => Vec::new(),
            },
            Pending::Operator {
                operator,
                count: pre,
            } => {
                let total = pre.max(1) * count.max(1);
                if is_doubled(operator, key) {
                    return self.linewise_operator(operator, total);
                }
                let Some(motion) = operator_motion(key, count) else {
                    return Vec::new();
                };
                let mut actions = Vec::new();
                let repeats = if motion_applies_once(motion) {
                    1
                } else {
                    total
                };
                for _ in 0..repeats {
                    actions.push(Action::Operate { operator, motion });
                }
                if operator == Operator::Change {
                    actions.extend(self.set_mode(Mode::Insert));
                }
                actions
            }
        }
    }

    fn linewise_operator(&mut self, operator: Operator, count: usize) -> Vec<Action> {
        match operator {
            Operator::Delete => vec![Action::DeleteLine(count)],
            Operator::Yank => vec![Action::YankLine(count)],
            Operator::Change => {
                let mut actions = vec![Action::ChangeLine(count)];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
        }
    }

    fn insert(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Escape => self.set_mode(Mode::Normal),
            Key::Enter => vec![Action::InsertChar('\n')],
            Key::Backspace => vec![Action::Move(Motion::Left), Action::DeleteChar],
            Key::Char(c) => vec![Action::InsertChar(c)],
            _ => Vec::new(),
        }
    }

    fn replace_mode(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Escape => self.set_mode(Mode::Normal),
            Key::Enter => vec![Action::InsertChar('\n')],
            Key::Backspace => vec![Action::Move(Motion::Left)],
            Key::Char(c) => vec![Action::ReplaceChar(c)],
            _ => Vec::new(),
        }
    }

    fn command(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Escape => self.set_mode(Mode::Normal),
            Key::Backspace => {
                self.command.pop();
                Vec::new()
            }
            Key::Enter => {
                let command = self.command.trim();
                let action = match command {
                    "w" => Some(Action::AppCommand(AppCommand::Save)),
                    "q" => Some(Action::AppCommand(AppCommand::Quit)),
                    "wq" | "x" => Some(Action::AppCommand(AppCommand::SaveQuit)),
                    _ => None,
                };
                self.command.clear();
                let mut actions = self.set_mode(Mode::Normal);
                if let Some(action) = action {
                    actions.push(action);
                }
                actions
            }
            Key::Char(c) => {
                self.command.push(c);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn search(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Escape => self.set_mode(Mode::Normal),
            Key::Backspace => {
                self.search.pop();
                Vec::new()
            }
            Key::Enter => {
                let query = self.search.clone();
                self.search.clear();
                let mut actions = self.set_mode(Mode::Normal);
                if !query.is_empty() {
                    self.last_search = Some(query.clone());
                    actions.push(Action::Search(query));
                }
                actions
            }
            Key::Char(c) => {
                self.search.push(c);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// Motion for a single-key motion shared by Normal and Visual modes.
fn motion_key(key: Key) -> Option<Motion> {
    Some(match key {
        Key::Char('h') => Motion::Left,
        Key::Char('l') => Motion::Right,
        Key::Char('j') => Motion::Line(LineDirection::Down),
        Key::Char('k') => Motion::Line(LineDirection::Up),
        Key::Char('w') => Motion::WordForward,
        Key::Char('b') => Motion::WordBackward,
        Key::Char('e') => Motion::WordEnd,
        Key::Char('0') => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonWhitespace,
        Key::Char('$') => Motion::LineEnd,
        _ => return None,
    })
}

/// Motion a `d`/`c`/`y` operator can consume — the shared motions plus `G`.
fn operator_motion(key: Key, count: usize) -> Option<Motion> {
    if let Key::Char('G') = key {
        return Some(jump_motion(count, Motion::FileEnd));
    }
    motion_key(key)
}

/// `{count}G`/`{count}gg` jump to a line; with no count they fall back to
/// `when_zero` (file end / file start).
fn jump_motion(count: usize, when_zero: Motion) -> Motion {
    if count > 0 {
        Motion::ToLine(count - 1)
    } else {
        when_zero
    }
}

/// True when `key` repeats the operator's own letter (`dd`, `cc`, `yy`).
fn is_doubled(operator: Operator, key: Key) -> bool {
    matches!(
        (operator, key),
        (Operator::Delete, Key::Char('d'))
            | (Operator::Change, Key::Char('c'))
            | (Operator::Yank, Key::Char('y'))
    )
}

/// File jumps act once regardless of count; other motions repeat per count.
fn motion_applies_once(motion: Motion) -> bool {
    matches!(
        motion,
        Motion::ToLine(_) | Motion::FileStart | Motion::FileEnd
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(input: &str) -> Vec<Key> {
        input.chars().map(Key::Char).collect()
    }

    fn feed(vim: &mut Vim, keys: impl IntoIterator<Item = Key>) -> Vec<Action> {
        keys.into_iter().flat_map(|key| vim.step(key)).collect()
    }

    #[test]
    fn normal_mode_motions_emit_actions() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("hjklwbe0^$G")),
            vec![
                Action::Move(Motion::Left),
                Action::Move(Motion::Line(LineDirection::Down)),
                Action::Move(Motion::Line(LineDirection::Up)),
                Action::Move(Motion::Right),
                Action::Move(Motion::WordForward),
                Action::Move(Motion::WordBackward),
                Action::Move(Motion::WordEnd),
                Action::Move(Motion::LineStart),
                Action::Move(Motion::FirstNonWhitespace),
                Action::Move(Motion::LineEnd),
                Action::Move(Motion::FileEnd),
            ]
        );
    }

    #[test]
    fn multi_key_normal_commands() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("gg")),
            vec![Action::Move(Motion::FileStart)]
        );
        assert_eq!(feed(&mut vim, chars("dd")), vec![Action::DeleteLine(1)]);
        assert_eq!(feed(&mut vim, chars("yy")), vec![Action::YankLine(1)]);
    }

    #[test]
    fn counts_repeat_motions() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("3l")),
            vec![
                Action::Move(Motion::Right),
                Action::Move(Motion::Right),
                Action::Move(Motion::Right),
            ]
        );
    }

    #[test]
    fn count_jumps_to_a_line() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("12G")),
            vec![Action::Move(Motion::ToLine(11))]
        );
        assert_eq!(
            feed(&mut vim, chars("3gg")),
            vec![Action::Move(Motion::ToLine(2))]
        );
    }

    #[test]
    fn leading_zero_is_a_motion_not_a_count() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("0")),
            vec![Action::Move(Motion::LineStart)]
        );
    }

    #[test]
    fn count_multiplies_a_linewise_operator() {
        let mut vim = Vim::new();
        assert_eq!(feed(&mut vim, chars("3dd")), vec![Action::DeleteLine(3)]);
        let mut vim = Vim::new();
        assert_eq!(feed(&mut vim, chars("2d3d")), vec![Action::DeleteLine(6)]);
    }

    #[test]
    fn operator_with_motion() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("dw")),
            vec![Action::Operate {
                operator: Operator::Delete,
                motion: Motion::WordForward,
            }]
        );
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("2de")),
            vec![
                Action::Operate {
                    operator: Operator::Delete,
                    motion: Motion::WordEnd,
                },
                Action::Operate {
                    operator: Operator::Delete,
                    motion: Motion::WordEnd,
                },
            ]
        );
    }

    #[test]
    fn change_operator_enters_insert_mode() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("cw")),
            vec![
                Action::Operate {
                    operator: Operator::Change,
                    motion: Motion::WordForward,
                },
                Action::EnterMode(Mode::Insert),
            ]
        );
        assert_eq!(vim.mode(), Mode::Insert);
    }

    #[test]
    fn visual_mode_selects_then_deletes() {
        let mut vim = Vim::new();
        let actions = feed(&mut vim, chars("vll"));
        assert_eq!(
            actions,
            vec![
                Action::EnterMode(Mode::Visual),
                Action::Move(Motion::Right),
                Action::Move(Motion::Right),
            ]
        );
        assert_eq!(vim.mode(), Mode::Visual);
        assert_eq!(
            feed(&mut vim, chars("d")),
            vec![Action::DeleteSelection, Action::EnterMode(Mode::Normal)]
        );
    }

    #[test]
    fn visual_line_mode_yanks() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("Vy")),
            vec![
                Action::EnterMode(Mode::VisualLine),
                Action::YankSelection,
                Action::EnterMode(Mode::Normal),
            ]
        );
    }

    #[test]
    fn replace_single_character() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("rz")),
            vec![Action::ReplaceChar('z'), Action::Move(Motion::Left)]
        );
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn replace_mode_overwrites_until_escape() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, [Key::Char('R'), Key::Char('a'), Key::Escape]),
            vec![
                Action::EnterMode(Mode::Replace),
                Action::ReplaceChar('a'),
                Action::EnterMode(Mode::Normal),
            ]
        );
    }

    #[test]
    fn insert_mode_emits_text_until_escape() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(
                &mut vim,
                [
                    Key::Char('i'),
                    Key::Char('a'),
                    Key::Char('b'),
                    Key::Enter,
                    Key::Escape
                ]
            ),
            vec![
                Action::EnterMode(Mode::Insert),
                Action::InsertChar('a'),
                Action::InsertChar('b'),
                Action::InsertChar('\n'),
                Action::EnterMode(Mode::Normal),
            ]
        );
        assert_eq!(vim.mode(), Mode::Normal);
    }

    #[test]
    fn append_and_open_line_enter_insert_with_setup_actions() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, chars("a")),
            vec![Action::Move(Motion::Right), Action::EnterMode(Mode::Insert)]
        );
        assert_eq!(
            feed(&mut vim, [Key::Escape]),
            vec![Action::EnterMode(Mode::Normal)]
        );
        assert_eq!(
            feed(&mut vim, chars("o")),
            vec![
                Action::Move(Motion::LineEnd),
                Action::InsertChar('\n'),
                Action::EnterMode(Mode::Insert)
            ]
        );
    }

    #[test]
    fn undo_redo_delete_paste() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(
                &mut vim,
                [
                    Key::Char('x'),
                    Key::Char('p'),
                    Key::Char('u'),
                    Key::Ctrl('r')
                ]
            ),
            vec![
                Action::DeleteChar,
                Action::Paste(PasteWhere::After),
                Action::Undo,
                Action::Redo,
            ]
        );
    }

    #[test]
    fn command_line_maps_save_quit_commands() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(&mut vim, [Key::Char(':'), Key::Char('w'), Key::Enter]),
            vec![
                Action::EnterMode(Mode::Command),
                Action::EnterMode(Mode::Normal),
                Action::AppCommand(AppCommand::Save)
            ]
        );
        assert_eq!(
            feed(&mut vim, [Key::Char(':'), Key::Char('q'), Key::Enter]),
            vec![
                Action::EnterMode(Mode::Command),
                Action::EnterMode(Mode::Normal),
                Action::AppCommand(AppCommand::Quit)
            ]
        );
        assert_eq!(
            feed(
                &mut vim,
                [Key::Char(':'), Key::Char('w'), Key::Char('q'), Key::Enter]
            ),
            vec![
                Action::EnterMode(Mode::Command),
                Action::EnterMode(Mode::Normal),
                Action::AppCommand(AppCommand::SaveQuit)
            ]
        );
    }

    #[test]
    fn slash_search_collects_query() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(
                &mut vim,
                [
                    Key::Char('/'),
                    Key::Char('n'),
                    Key::Char('e'),
                    Key::Char('e'),
                    Key::Char('d'),
                    Key::Enter
                ]
            ),
            vec![
                Action::EnterMode(Mode::Search),
                Action::EnterMode(Mode::Normal),
                Action::Search("need".to_string())
            ]
        );
    }

    #[test]
    fn search_repeat_reuses_the_last_query() {
        let mut vim = Vim::new();
        assert_eq!(
            feed(
                &mut vim,
                [
                    Key::Char('/'),
                    Key::Char('f'),
                    Key::Char('o'),
                    Key::Char('o'),
                    Key::Enter,
                    Key::Char('n'),
                    Key::Char('N'),
                ]
            ),
            vec![
                Action::EnterMode(Mode::Search),
                Action::EnterMode(Mode::Normal),
                Action::Search("foo".to_string()),
                Action::Search("foo".to_string()),
                Action::SearchPrevious("foo".to_string()),
            ]
        );
    }

    #[test]
    fn escape_cancels_a_pending_operator() {
        let mut vim = Vim::new();
        assert_eq!(feed(&mut vim, [Key::Char('d'), Key::Escape]), Vec::new());
        assert_eq!(vim.mode(), Mode::Normal);
    }
}
