//! The hotkey bus abstraction and its headless fake.
//!
//! Core code registers chords through the [`HotkeyBus`] trait and never touches
//! the OS directly (AGENTS §3). The real `global-hotkey` backend implements the
//! same trait behind `ui-smoke`; tests drive [`FakeHotkeyBus`], which models
//! both an external app owning a chord (so conflict detection fires) and the
//! OS delivering key presses.

use std::collections::HashMap;
use std::collections::HashSet;

use crate::chord::Hotkey;
use crate::error::HotkeyError;

/// Opaque handle to a registered hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HotkeyId(pub u32);

/// Register and release global hotkeys. Registration is fallible: a chord may
/// already be owned by another application or already registered here.
pub trait HotkeyBus {
    /// Register `chord`, returning a handle. Errors with
    /// [`HotkeyError::Conflict`] if the chord is already taken.
    fn register(&mut self, chord: &str) -> Result<HotkeyId, HotkeyError>;

    /// Release a previously-registered hotkey. A no-op for unknown ids.
    fn unregister(&mut self, id: HotkeyId);
}

/// In-memory hotkey bus for tests. Tracks registrations, simulates other apps
/// owning chords, and records "presses".
#[derive(Debug, Default)]
pub struct FakeHotkeyBus {
    owned_externally: HashSet<Hotkey>,
    registered: HashMap<HotkeyId, Hotkey>,
    next_id: u32,
    fired: Vec<HotkeyId>,
}

impl FakeHotkeyBus {
    /// An empty bus with no external conflicts.
    pub fn new() -> Self {
        Self::default()
    }

    /// A bus where each chord in `chords` is already owned by another app, so
    /// registering it returns [`HotkeyError::Conflict`].
    pub fn with_external_conflicts<I, S>(chords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let owned_externally = chords
            .into_iter()
            .filter_map(|c| Hotkey::parse(c.as_ref()).ok())
            .collect();
        FakeHotkeyBus {
            owned_externally,
            ..Default::default()
        }
    }

    /// `true` if `hotkey` is currently registered on this bus.
    pub fn is_registered(&self, hotkey: &Hotkey) -> bool {
        self.registered.values().any(|h| h == hotkey)
    }

    /// Simulate the OS delivering a press of `chord`. If a matching hotkey is
    /// registered, records it as fired and returns its id.
    pub fn simulate_press(&mut self, chord: &str) -> Option<HotkeyId> {
        let hotkey = Hotkey::parse(chord).ok()?;
        let id = self
            .registered
            .iter()
            .find(|(_, h)| **h == hotkey)
            .map(|(id, _)| *id)?;
        self.fired.push(id);
        Some(id)
    }

    /// Drain the recorded presses since the last drain.
    pub fn drain_fired(&mut self) -> Vec<HotkeyId> {
        std::mem::take(&mut self.fired)
    }
}

impl HotkeyBus for FakeHotkeyBus {
    fn register(&mut self, chord: &str) -> Result<HotkeyId, HotkeyError> {
        let hotkey = Hotkey::parse(chord)?;
        if self.owned_externally.contains(&hotkey) || self.is_registered(&hotkey) {
            return Err(HotkeyError::Conflict {
                chord: hotkey.display(),
                hotkey,
            });
        }
        let id = HotkeyId(self.next_id);
        self.next_id += 1;
        self.registered.insert(id, hotkey);
        Ok(id)
    }

    fn unregister(&mut self, id: HotkeyId) {
        self.registered.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_returns_distinct_ids() {
        let mut bus = FakeHotkeyBus::new();
        let a = bus.register("Ctrl+Alt+A").unwrap();
        let b = bus.register("Ctrl+Alt+B").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn external_conflict_is_reported_not_swallowed() {
        let mut bus = FakeHotkeyBus::with_external_conflicts(["Ctrl+O"]);
        let err = bus.register("ctrl+o").unwrap_err();
        match err {
            HotkeyError::Conflict { chord, .. } => assert_eq!(chord, "Ctrl+O"),
            other => panic!("expected conflict, got {other:?}"),
        }
    }

    #[test]
    fn double_registration_conflicts() {
        let mut bus = FakeHotkeyBus::new();
        bus.register("Ctrl+Alt+A").unwrap();
        assert!(matches!(
            bus.register("alt+ctrl+a"),
            Err(HotkeyError::Conflict { .. })
        ));
    }

    #[test]
    fn unregister_frees_the_chord() {
        let mut bus = FakeHotkeyBus::new();
        let id = bus.register("Ctrl+Alt+A").unwrap();
        bus.unregister(id);
        // Now re-registering the same chord succeeds.
        assert!(bus.register("Ctrl+Alt+A").is_ok());
    }

    #[test]
    fn presses_fire_only_for_registered_chords() {
        let mut bus = FakeHotkeyBus::new();
        let id = bus.register("Ctrl+Alt+A").unwrap();
        assert_eq!(bus.simulate_press("ctrl+alt+a"), Some(id));
        assert_eq!(bus.simulate_press("Ctrl+Alt+Z"), None); // not registered
        assert_eq!(bus.drain_fired(), vec![id]);
        assert!(bus.drain_fired().is_empty()); // drained
    }
}
