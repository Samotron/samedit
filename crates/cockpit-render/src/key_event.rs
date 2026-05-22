//! Input event → [`KeyChord`] mapping (spec §12 / M1.10).
//!
//! A small device-agnostic [`KeyEvent`] representation that the windowing
//! layer (`winit`, today; anything else tomorrow) translates into, plus
//! [`event_to_chord`] which produces the [`KeyChord`] consumed by
//! `cockpit-commands` and the `cockpit-ui` `InputRouter`.
//!
//! Keeping this module pure means the mapping is unit-tested without ever
//! opening a window — the binary is responsible for the winit ↔ [`KeyEvent`]
//! translation when the event loop lands (M1.16).
//!
//! Spelling conventions for the produced chords:
//!
//! - Printable characters use the **lower-case** form of the typed character
//!   (`Shift+p` → `"p"`, not `"P"`). This matches the existing
//!   [`KeyChord`] parser used for keybinding config strings.
//! - Named keys keep their canonical spelling (`"Escape"`, `"Enter"`, `"Tab"`,
//!   `"Backspace"`, `"F1"`, `"PageUp"`, `Arrow{Up,Down,Left,Right}`, etc.).

use cockpit_commands::{KeyChord, KeyStroke, Modifiers};

/// One keyboard event coming from the windowing layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: LogicalKey,
    pub modifiers: KeyModifiers,
    /// True when the event represents a key press (vs release / repeat-only).
    pub pressed: bool,
}

impl KeyEvent {
    /// Construct a pressed key event with no modifiers.
    pub fn pressed(key: LogicalKey) -> Self {
        Self {
            key,
            modifiers: KeyModifiers::default(),
            pressed: true,
        }
    }

    /// Add modifiers to a pressed event in a fluent style.
    pub fn with_modifiers(mut self, modifiers: KeyModifiers) -> Self {
        self.modifiers = modifiers;
        self
    }
}

/// Logical (post-layout) key identity coming from the windowing layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalKey {
    /// A character key. The character is the post-layout, post-shift value
    /// (e.g. typing `Shift+a` on a US layout produces `'A'`).
    Char(char),
    /// A named, non-character key (Escape, Enter, F1, …).
    Named(NamedKey),
}

/// Modifier-key flags as reported by the windowing layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl KeyModifiers {
    /// True when any modifier flag is set.
    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }
}

/// Named keys that do not produce a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Space,
    F(u8),
}

impl NamedKey {
    /// Canonical key label used in [`KeyChord`] strings.
    pub fn label(self) -> String {
        match self {
            Self::Escape => "Escape".into(),
            Self::Enter => "Enter".into(),
            Self::Tab => "Tab".into(),
            Self::Backspace => "Backspace".into(),
            Self::Delete => "Delete".into(),
            Self::Insert => "Insert".into(),
            Self::Home => "Home".into(),
            Self::End => "End".into(),
            Self::PageUp => "PageUp".into(),
            Self::PageDown => "PageDown".into(),
            Self::ArrowUp => "ArrowUp".into(),
            Self::ArrowDown => "ArrowDown".into(),
            Self::ArrowLeft => "ArrowLeft".into(),
            Self::ArrowRight => "ArrowRight".into(),
            Self::Space => "Space".into(),
            Self::F(n) => format!("F{n}"),
        }
    }
}

/// Translate a [`KeyEvent`] into a single-stroke [`KeyChord`].
///
/// Returns [`None`] for non-press events (so the caller can keep the chord
/// dispatch on key-down only, matching spec §12 where the global router is
/// invoked once per press).
pub fn event_to_chord(event: &KeyEvent) -> Option<KeyChord> {
    if !event.pressed {
        return None;
    }

    let (label, drop_shift) = match &event.key {
        LogicalKey::Char(c) => {
            let lower: String = c.to_lowercase().collect();
            // For Char(' '), prefer the named "Space" label so chords are readable.
            if lower == " " {
                (NamedKey::Space.label(), false)
            } else {
                // If the original character only differs from its lower-case by
                // case (i.e. the user typed `Shift+a` → `A`), the shift modifier
                // is implied by the character; drop it to keep `Ctrl+Shift+p`
                // bindings matching `Shift+p` → `"p"` only when the user
                // pressed Shift explicitly. Letters always need shift squashed.
                let drop_shift = c.is_alphabetic();
                (lower, drop_shift)
            }
        }
        LogicalKey::Named(name) => (name.label(), false),
    };

    let mut modifiers = Modifiers {
        ctrl: event.modifiers.ctrl,
        alt: event.modifiers.alt,
        shift: event.modifiers.shift,
        meta: event.modifiers.meta,
    };
    if drop_shift {
        // Letter chords: keep `shift` only when the original char was already
        // uppercase (user pressed Shift to produce it).
        let original_upper = matches!(event.key, LogicalKey::Char(c) if c.is_uppercase());
        modifiers.shift = original_upper || event.modifiers.shift;
    }

    Some(KeyChord::new(vec![KeyStroke::new(label, modifiers)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(input: &str) -> KeyChord {
        input.parse().expect("chord parses")
    }

    #[test]
    fn release_events_produce_no_chord() {
        let event = KeyEvent {
            key: LogicalKey::Char('a'),
            modifiers: KeyModifiers::default(),
            pressed: false,
        };
        assert_eq!(event_to_chord(&event), None);
    }

    #[test]
    fn plain_lowercase_letter_maps_to_modifier_free_chord() {
        let event = KeyEvent::pressed(LogicalKey::Char('a'));
        assert_eq!(event_to_chord(&event), Some(chord("a")));
    }

    #[test]
    fn shifted_letter_round_trips_through_parser() {
        // User typed Shift+P → the OS reports key='P' with shift held.
        let event = KeyEvent::pressed(LogicalKey::Char('P')).with_modifiers(KeyModifiers {
            shift: true,
            ..KeyModifiers::default()
        });
        assert_eq!(event_to_chord(&event), Some(chord("Shift+p")));
    }

    #[test]
    fn ctrl_shift_p_matches_command_palette_binding() {
        let event = KeyEvent::pressed(LogicalKey::Char('P')).with_modifiers(KeyModifiers {
            ctrl: true,
            shift: true,
            ..KeyModifiers::default()
        });
        assert_eq!(event_to_chord(&event), Some(chord("Ctrl+Shift+p")));
    }

    #[test]
    fn ctrl_s_matches_save_binding() {
        let event = KeyEvent::pressed(LogicalKey::Char('s')).with_modifiers(KeyModifiers {
            ctrl: true,
            ..KeyModifiers::default()
        });
        assert_eq!(event_to_chord(&event), Some(chord("Ctrl+s")));
    }

    #[test]
    fn ctrl_backtick_toggles_terminal() {
        let event = KeyEvent::pressed(LogicalKey::Char('`')).with_modifiers(KeyModifiers {
            ctrl: true,
            ..KeyModifiers::default()
        });
        assert_eq!(event_to_chord(&event), Some(chord("Ctrl+`")));
    }

    #[test]
    fn named_keys_use_canonical_labels() {
        assert_eq!(
            event_to_chord(&KeyEvent::pressed(LogicalKey::Named(NamedKey::Escape))),
            Some(chord("Escape"))
        );
        assert_eq!(
            event_to_chord(&KeyEvent::pressed(LogicalKey::Named(NamedKey::Enter))),
            Some(chord("Enter"))
        );
        assert_eq!(
            event_to_chord(&KeyEvent::pressed(LogicalKey::Named(NamedKey::F(7)))),
            Some(chord("F7"))
        );
        assert_eq!(
            event_to_chord(&KeyEvent::pressed(LogicalKey::Named(NamedKey::ArrowDown))),
            Some(chord("ArrowDown"))
        );
    }

    #[test]
    fn space_char_maps_to_named_space_label() {
        let event = KeyEvent::pressed(LogicalKey::Char(' '));
        assert_eq!(event_to_chord(&event), Some(chord("Space")));
    }

    #[test]
    fn alt_named_key_preserves_modifier() {
        let event = KeyEvent::pressed(LogicalKey::Named(NamedKey::ArrowLeft)).with_modifiers(
            KeyModifiers {
                alt: true,
                ..KeyModifiers::default()
            },
        );
        assert_eq!(event_to_chord(&event), Some(chord("Alt+ArrowLeft")));
    }
}
