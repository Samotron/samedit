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

/// Buffer or app action produced by a key transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    MoveLeft,
    MoveRight,
    MoveLine(LineDirection),
    MoveWordForward,
    MoveWordBackward,
    MoveWordEnd,
    MoveLineStart,
    MoveFirstNonWhitespace,
    MoveLineEnd,
    MoveFileStart,
    MoveFileEnd,
    EnterMode(Mode),
    InsertChar(char),
    DeleteChar,
    DeleteLine,
    YankLine,
    PasteAfter,
    Undo,
    Redo,
    Search(String),
    AppCommand(AppCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    G,
    Delete,
    Yank,
}

/// Pure Vim input state.
#[derive(Debug, Clone, Default)]
pub struct Vim {
    mode: Mode,
    pending: Option<Pending>,
    command: String,
    search: String,
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
            Mode::Normal => self.normal(key),
            Mode::Insert => self.insert(key),
            Mode::Command => self.command(key),
            Mode::Search => self.search(key),
        }
    }

    fn set_mode(&mut self, mode: Mode) -> Vec<Action> {
        self.mode = mode;
        self.pending = None;
        vec![Action::EnterMode(mode)]
    }

    fn normal(&mut self, key: Key) -> Vec<Action> {
        if let Some(pending) = self.pending.take() {
            return self.complete_pending(pending, key);
        }

        match key {
            Key::Char('h') => vec![Action::MoveLeft],
            Key::Char('j') => vec![Action::MoveLine(LineDirection::Down)],
            Key::Char('k') => vec![Action::MoveLine(LineDirection::Up)],
            Key::Char('l') => vec![Action::MoveRight],
            Key::Char('w') => vec![Action::MoveWordForward],
            Key::Char('b') => vec![Action::MoveWordBackward],
            Key::Char('e') => vec![Action::MoveWordEnd],
            Key::Char('0') => vec![Action::MoveLineStart],
            Key::Char('^') => vec![Action::MoveFirstNonWhitespace],
            Key::Char('$') => vec![Action::MoveLineEnd],
            Key::Char('G') => vec![Action::MoveFileEnd],
            Key::Char('i') => self.set_mode(Mode::Insert),
            Key::Char('a') => {
                let mut actions = vec![Action::MoveRight];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('o') => {
                let mut actions = vec![Action::MoveLineEnd, Action::InsertChar('\n')];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('O') => {
                let mut actions = vec![
                    Action::MoveLineStart,
                    Action::InsertChar('\n'),
                    Action::MoveLeft,
                ];
                actions.extend(self.set_mode(Mode::Insert));
                actions
            }
            Key::Char('x') => vec![Action::DeleteChar],
            Key::Char('u') => vec![Action::Undo],
            Key::Ctrl('r') | Key::Ctrl('R') => vec![Action::Redo],
            Key::Char('p') => vec![Action::PasteAfter],
            Key::Char('g') => {
                self.pending = Some(Pending::G);
                Vec::new()
            }
            Key::Char('d') => {
                self.pending = Some(Pending::Delete);
                Vec::new()
            }
            Key::Char('y') => {
                self.pending = Some(Pending::Yank);
                Vec::new()
            }
            Key::Char(':') => {
                self.command.clear();
                self.set_mode(Mode::Command)
            }
            Key::Char('/') => {
                self.search.clear();
                self.set_mode(Mode::Search)
            }
            Key::Escape => {
                self.pending = None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn complete_pending(&mut self, pending: Pending, key: Key) -> Vec<Action> {
        match (pending, key) {
            (Pending::G, Key::Char('g')) => vec![Action::MoveFileStart],
            (Pending::Delete, Key::Char('d')) => vec![Action::DeleteLine],
            (Pending::Yank, Key::Char('y')) => vec![Action::YankLine],
            _ => Vec::new(),
        }
    }

    fn insert(&mut self, key: Key) -> Vec<Action> {
        match key {
            Key::Escape => self.set_mode(Mode::Normal),
            Key::Enter => vec![Action::InsertChar('\n')],
            Key::Backspace => vec![Action::MoveLeft, Action::DeleteChar],
            Key::Char(c) => vec![Action::InsertChar(c)],
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
                Action::MoveLeft,
                Action::MoveLine(LineDirection::Down),
                Action::MoveLine(LineDirection::Up),
                Action::MoveRight,
                Action::MoveWordForward,
                Action::MoveWordBackward,
                Action::MoveWordEnd,
                Action::MoveLineStart,
                Action::MoveFirstNonWhitespace,
                Action::MoveLineEnd,
                Action::MoveFileEnd,
            ]
        );
    }

    #[test]
    fn multi_key_normal_commands() {
        let mut vim = Vim::new();
        assert_eq!(feed(&mut vim, chars("gg")), vec![Action::MoveFileStart]);
        assert_eq!(feed(&mut vim, chars("dd")), vec![Action::DeleteLine]);
        assert_eq!(feed(&mut vim, chars("yy")), vec![Action::YankLine]);
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
            vec![Action::MoveRight, Action::EnterMode(Mode::Insert)]
        );
        assert_eq!(
            feed(&mut vim, [Key::Escape]),
            vec![Action::EnterMode(Mode::Normal)]
        );
        assert_eq!(
            feed(&mut vim, chars("o")),
            vec![
                Action::MoveLineEnd,
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
                Action::PasteAfter,
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
}
