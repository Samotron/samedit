//! `cockpit-commands` — the command system.
//!
//! A single registry of commands that keybindings, the command palette, and
//! tests all dispatch through (spec §16).

use std::collections::{BTreeMap, btree_map::Entry};
use std::fmt;
use std::str::FromStr;

/// Stable command identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(String);

impl CommandId {
    /// Create a command id. IDs are usually dot-separated, e.g.
    /// `editor.save` or `pane.focus_left`.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Borrow the id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CommandId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Command execution context.
#[derive(Debug, Default)]
pub struct CommandContext {
    log: Vec<CommandId>,
}

impl CommandContext {
    /// Record a command execution. Real app state is wired in later; the log is
    /// useful today for deterministic registry tests and debug surfaces.
    pub fn record(&mut self, id: impl Into<CommandId>) {
        self.log.push(id.into());
    }

    /// Recorded command executions.
    pub fn log(&self) -> &[CommandId] {
        &self.log
    }
}

type Handler = Box<dyn FnMut(&mut CommandContext) + Send + 'static>;

/// A registered command.
pub struct Command {
    id: CommandId,
    title: String,
    handler: Handler,
}

impl Command {
    /// Construct a command from an id, palette title, and handler.
    pub fn new(
        id: impl Into<CommandId>,
        title: impl Into<String>,
        handler: impl FnMut(&mut CommandContext) + Send + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            handler: Box::new(handler),
        }
    }

    /// Command id.
    pub fn id(&self) -> &CommandId {
        &self.id
    }

    /// Human-readable command title.
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// Errors returned by the command system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    DuplicateCommand(CommandId),
    UnknownCommand(CommandId),
    DuplicateKeybinding(KeyChord),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateCommand(id) => write!(f, "duplicate command `{id}`"),
            Self::UnknownCommand(id) => write!(f, "unknown command `{id}`"),
            Self::DuplicateKeybinding(chord) => write!(f, "duplicate keybinding `{chord}`"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Registry of known commands.
#[derive(Default)]
pub struct Registry {
    commands: BTreeMap<CommandId, Command>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a command.
    pub fn register(&mut self, command: Command) -> Result<(), CommandError> {
        match self.commands.entry(command.id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(command);
                Ok(())
            }
            Entry::Occupied(entry) => Err(CommandError::DuplicateCommand(entry.key().clone())),
        }
    }

    /// Return a command by id.
    pub fn get(&self, id: &CommandId) -> Option<&Command> {
        self.commands.get(id)
    }

    /// Iterate commands in stable id order.
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.commands.values()
    }

    /// Dispatch a command by id.
    pub fn dispatch(
        &mut self,
        id: &CommandId,
        context: &mut CommandContext,
    ) -> Result<(), CommandError> {
        let Some(command) = self.commands.get_mut(id) else {
            return Err(CommandError::UnknownCommand(id.clone()));
        };
        (command.handler)(context);
        Ok(())
    }
}

/// Keyboard modifiers for a key stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    /// Ctrl only.
    pub const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };

    /// Ctrl+Shift.
    pub const CTRL_SHIFT: Self = Self {
        ctrl: true,
        shift: true,
        ..Self::NONE
    };
}

/// One key press in a keybinding chord.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyStroke {
    key: String,
    modifiers: Modifiers,
}

impl KeyStroke {
    /// Create a key stroke.
    pub fn new(key: impl Into<String>, modifiers: Modifiers) -> Self {
        Self {
            key: key.into(),
            modifiers,
        }
    }

    /// Key label for this stroke.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Stroke modifiers.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }
}

/// A command keybinding. Multi-stroke chords are supported for future use.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyChord(Vec<KeyStroke>);

impl KeyChord {
    /// Create a chord from one or more strokes.
    pub fn new(strokes: impl Into<Vec<KeyStroke>>) -> Self {
        Self(strokes.into())
    }

    /// Create a single-stroke chord.
    pub fn single(key: impl Into<String>, modifiers: Modifiers) -> Self {
        Self::new(vec![KeyStroke::new(key, modifiers)])
    }

    /// Strokes in this chord.
    pub fn strokes(&self) -> &[KeyStroke] {
        &self.0
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, stroke) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            if stroke.modifiers.ctrl {
                f.write_str("Ctrl+")?;
            }
            if stroke.modifiers.alt {
                f.write_str("Alt+")?;
            }
            if stroke.modifiers.shift {
                f.write_str("Shift+")?;
            }
            if stroke.modifiers.meta {
                f.write_str("Meta+")?;
            }
            f.write_str(&stroke.key)?;
        }
        Ok(())
    }
}

impl FromStr for KeyChord {
    type Err = KeyChordParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut strokes = Vec::new();
        for raw_stroke in input.split_whitespace() {
            let mut modifiers = Modifiers::NONE;
            let mut key = None;

            for part in raw_stroke.split('+') {
                let part = part.trim();
                if part.is_empty() {
                    return Err(KeyChordParseError::EmptyPart);
                }

                match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => modifiers.ctrl = true,
                    "alt" | "option" => modifiers.alt = true,
                    "shift" => modifiers.shift = true,
                    "meta" | "cmd" | "command" | "super" => modifiers.meta = true,
                    _ => {
                        if key.replace(part.to_string()).is_some() {
                            return Err(KeyChordParseError::MultipleKeys(raw_stroke.to_string()));
                        }
                    }
                }
            }

            let Some(key) = key else {
                return Err(KeyChordParseError::MissingKey(raw_stroke.to_string()));
            };
            strokes.push(KeyStroke::new(key, modifiers));
        }

        if strokes.is_empty() {
            return Err(KeyChordParseError::EmptyChord);
        }

        Ok(KeyChord::new(strokes))
    }
}

/// Key chord parse error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChordParseError {
    EmptyChord,
    EmptyPart,
    MissingKey(String),
    MultipleKeys(String),
}

impl fmt::Display for KeyChordParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyChord => f.write_str("empty key chord"),
            Self::EmptyPart => f.write_str("empty key chord segment"),
            Self::MissingKey(stroke) => write!(f, "key chord stroke `{stroke}` has no key"),
            Self::MultipleKeys(stroke) => {
                write!(f, "key chord stroke `{stroke}` has multiple keys")
            }
        }
    }
}

impl std::error::Error for KeyChordParseError {}

/// Keybinding resolver.
#[derive(Debug, Default)]
pub struct Keymap {
    bindings: BTreeMap<KeyChord, CommandId>,
}

impl Keymap {
    /// Create an empty keymap.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `chord` to `command`.
    pub fn bind(
        &mut self,
        chord: KeyChord,
        command: impl Into<CommandId>,
    ) -> Result<(), CommandError> {
        match self.bindings.entry(chord) {
            Entry::Vacant(slot) => {
                slot.insert(command.into());
                Ok(())
            }
            Entry::Occupied(entry) => Err(CommandError::DuplicateKeybinding(entry.key().clone())),
        }
    }

    /// Resolve a chord to a command id.
    pub fn resolve(&self, chord: &KeyChord) -> Option<&CommandId> {
        self.bindings.get(chord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_dispatches_command() {
        let mut registry = Registry::new();
        registry
            .register(Command::new("editor.save", "Editor: Save", |ctx| {
                ctx.record("editor.save");
            }))
            .unwrap();

        let mut context = CommandContext::default();
        registry
            .dispatch(&CommandId::from("editor.save"), &mut context)
            .unwrap();

        assert_eq!(context.log(), &[CommandId::from("editor.save")]);
    }

    #[test]
    fn rejects_duplicate_command_ids() {
        let mut registry = Registry::new();
        registry
            .register(Command::new("editor.save", "Editor: Save", |_| {}))
            .unwrap();

        assert_eq!(
            registry.register(Command::new("editor.save", "Save Again", |_| {})),
            Err(CommandError::DuplicateCommand(CommandId::from(
                "editor.save"
            )))
        );
    }

    #[test]
    fn unknown_dispatch_is_error() {
        let mut registry = Registry::new();
        let mut context = CommandContext::default();
        assert_eq!(
            registry.dispatch(&CommandId::from("missing"), &mut context),
            Err(CommandError::UnknownCommand(CommandId::from("missing")))
        );
    }

    #[test]
    fn resolves_keybinding() {
        let mut keymap = Keymap::new();
        let chord = KeyChord::single("s", Modifiers::CTRL);
        keymap.bind(chord.clone(), "editor.save").unwrap();
        assert_eq!(
            keymap.resolve(&chord),
            Some(&CommandId::from("editor.save"))
        );
    }

    #[test]
    fn parses_key_chords_from_config_strings() {
        let chord: KeyChord = "Ctrl+Shift+p".parse().unwrap();
        assert_eq!(chord, KeyChord::single("p", Modifiers::CTRL_SHIFT));

        let chord: KeyChord = "Ctrl+`".parse().unwrap();
        assert_eq!(chord, KeyChord::single("`", Modifiers::CTRL));
    }

    #[test]
    fn rejects_invalid_key_chords() {
        assert_eq!("".parse::<KeyChord>(), Err(KeyChordParseError::EmptyChord));
        assert_eq!(
            "Ctrl+Shift".parse::<KeyChord>(),
            Err(KeyChordParseError::MissingKey("Ctrl+Shift".to_string()))
        );
        assert_eq!(
            "Ctrl+p+q".parse::<KeyChord>(),
            Err(KeyChordParseError::MultipleKeys("Ctrl+p+q".to_string()))
        );
    }

    #[test]
    fn rejects_duplicate_keybindings() {
        let mut keymap = Keymap::new();
        let chord = KeyChord::single("p", Modifiers::CTRL_SHIFT);
        keymap.bind(chord.clone(), "palette.open").unwrap();
        assert_eq!(
            keymap.bind(chord.clone(), "project.open_file"),
            Err(CommandError::DuplicateKeybinding(chord))
        );
    }
}
