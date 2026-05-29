//! Chord grammar: parse `Ctrl+Alt+J`-style strings into typed [`Hotkey`]s.
//!
//! Parsing is case-insensitive and modifier aliases are normalised (so
//! `cmd`/`super`/`win`/`meta` all mean the platform "super" key). A parsed
//! hotkey compares by value, which is what conflict detection relies on:
//! `Ctrl+Alt+J` and `alt+control+j` are the same hotkey.

use crate::error::HotkeyError;

/// The modifier set of a chord.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The "super" key — Command on macOS, Windows/Super elsewhere.
    pub super_: bool,
}

impl Modifiers {
    /// `true` if no modifier is set.
    pub fn is_empty(self) -> bool {
        !(self.ctrl || self.alt || self.shift || self.super_)
    }
}

/// The non-modifier key of a chord, normalised.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    /// A character key (letters upper-cased, so `j` and `J` match).
    Char(char),
    /// A function key, `F1`..=`F24`.
    Function(u8),
    /// A named key (Space, Enter, Tab, …).
    Named(NamedKey),
}

/// Named non-character keys we recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    Space,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

/// A fully-parsed hotkey: a modifier set plus one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Modifiers,
    pub key: Key,
}

impl Hotkey {
    /// Parse a chord string such as `"Ctrl+Alt+J"`.
    pub fn parse(chord: &str) -> Result<Hotkey, HotkeyError> {
        let raw = chord.trim();
        if raw.is_empty() {
            return Err(HotkeyError::EmptyChord);
        }

        let tokens: Vec<&str> = raw
            .split('+')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() {
            return Err(HotkeyError::EmptyChord);
        }

        let mut mods = Modifiers::default();
        // Every token but the last must be a modifier; the last is the key.
        let (key_tok, mod_toks) = tokens.split_last().expect("non-empty");
        for tok in mod_toks {
            apply_modifier(&mut mods, tok)
                .ok_or_else(|| HotkeyError::UnknownToken((*tok).to_string()))?;
        }

        // A trailing modifier name (e.g. "Ctrl+Alt") means no key was given.
        if is_modifier(key_tok) {
            return Err(HotkeyError::MissingKey(raw.to_string()));
        }
        let key = parse_key(key_tok).ok_or_else(|| HotkeyError::UnknownKey(key_tok.to_string()))?;

        Ok(Hotkey { mods, key })
    }

    /// Canonical display form, e.g. `"Ctrl+Alt+J"`. Stable for config/UI.
    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.mods.alt {
            parts.push("Alt".to_string());
        }
        if self.mods.shift {
            parts.push("Shift".to_string());
        }
        if self.mods.super_ {
            parts.push("Super".to_string());
        }
        parts.push(key_display(&self.key));
        parts.join("+")
    }
}

fn apply_modifier(mods: &mut Modifiers, token: &str) -> Option<()> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => mods.ctrl = true,
        "alt" | "opt" | "option" => mods.alt = true,
        "shift" => mods.shift = true,
        "super" | "cmd" | "command" | "win" | "meta" => mods.super_ = true,
        _ => return None,
    }
    Some(())
}

fn is_modifier(token: &str) -> bool {
    let mut probe = Modifiers::default();
    apply_modifier(&mut probe, token).is_some()
}

fn parse_key(token: &str) -> Option<Key> {
    // Single character.
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        return Some(Key::Char(c.to_ascii_uppercase()));
    }

    // Function key.
    if let Some(rest) = token
        .strip_prefix(['F', 'f'])
        .filter(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()))
        && let Ok(n) = rest.parse::<u8>()
        && (1..=24).contains(&n)
    {
        return Some(Key::Function(n));
    }

    let named = match token.to_ascii_lowercase().as_str() {
        "space" => NamedKey::Space,
        "enter" | "return" => NamedKey::Enter,
        "tab" => NamedKey::Tab,
        "esc" | "escape" => NamedKey::Escape,
        "backspace" => NamedKey::Backspace,
        "delete" | "del" => NamedKey::Delete,
        "insert" | "ins" => NamedKey::Insert,
        "up" => NamedKey::Up,
        "down" => NamedKey::Down,
        "left" => NamedKey::Left,
        "right" => NamedKey::Right,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "pgup" => NamedKey::PageUp,
        "pagedown" | "pgdn" => NamedKey::PageDown,
        _ => return None,
    };
    Some(Key::Named(named))
}

fn key_display(key: &Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Function(n) => format!("F{n}"),
        Key::Named(n) => format!("{n:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifiers_and_key() {
        let hk = Hotkey::parse("Ctrl+Alt+J").unwrap();
        assert!(hk.mods.ctrl && hk.mods.alt && !hk.mods.shift && !hk.mods.super_);
        assert_eq!(hk.key, Key::Char('J'));
    }

    #[test]
    fn is_case_and_alias_insensitive() {
        let a = Hotkey::parse("Ctrl+Alt+J").unwrap();
        let b = Hotkey::parse("alt + control + j").unwrap();
        assert_eq!(a, b);
        // cmd/win/meta all fold to super.
        assert_eq!(Hotkey::parse("Cmd+Space"), Hotkey::parse("Super+space"));
        assert_eq!(Hotkey::parse("Win+Space"), Hotkey::parse("Meta+space"));
    }

    #[test]
    fn function_and_named_keys() {
        assert_eq!(Hotkey::parse("Ctrl+F12").unwrap().key, Key::Function(12));
        assert_eq!(
            Hotkey::parse("Ctrl+Space").unwrap().key,
            Key::Named(NamedKey::Space)
        );
        assert_eq!(
            Hotkey::parse("Alt+Return").unwrap().key,
            Key::Named(NamedKey::Enter)
        );
    }

    #[test]
    fn display_is_canonical() {
        assert_eq!(
            Hotkey::parse("alt+control+j").unwrap().display(),
            "Ctrl+Alt+J"
        );
        assert_eq!(Hotkey::parse("cmd+space").unwrap().display(), "Super+Space");
        assert_eq!(Hotkey::parse("ctrl+f5").unwrap().display(), "Ctrl+F5");
    }

    #[test]
    fn errors() {
        assert!(matches!(Hotkey::parse(""), Err(HotkeyError::EmptyChord)));
        assert!(matches!(
            Hotkey::parse("Ctrl+Alt"),
            Err(HotkeyError::MissingKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("Hyper+J"),
            Err(HotkeyError::UnknownToken(_))
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl+Nope"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(matches!(
            Hotkey::parse("Ctrl+F99"),
            Err(HotkeyError::UnknownKey(_))
        ));
    }
}
