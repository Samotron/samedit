//! `cockpit-tray` — system-tray menu model, event routing, and a headless fake
//! tray (v0.12 M12.2).
//!
//! The jot app (and the v0.13 launcher) live in the system tray. This crate
//! owns the *backend-free* half: the [`Menu`] model, the [`TrayEvent`] stream
//! (left-click, menu activation), and the [`Tray`] seam that core code drives
//! instead of touching the OS (AGENTS §3).
//!
//! The real `tray-icon` backend is a follow-up behind the `ui-smoke` feature —
//! it needs a windowing/tray CI leg to smoke-test, so it ships when it can be
//! verified. [`FakeTray`] models the tray in memory for the fast test leg.

pub mod menu;

pub use menu::{Menu, MenuItem, MenuItemId};

/// Something the user did with the tray.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayEvent {
    /// The icon was left-clicked (the jot app opens the capture picker).
    LeftClick,
    /// A menu item was activated.
    MenuItem(MenuItemId),
}

/// The tray seam. Core code sets the menu/tooltip; the backend surfaces clicks
/// as [`TrayEvent`]s.
pub trait Tray {
    /// Replace the tray menu.
    fn set_menu(&mut self, menu: Menu);
    /// Set the hover tooltip.
    fn set_tooltip(&mut self, tooltip: &str);
}

/// In-memory tray for tests: records the menu/tooltip and lets tests simulate
/// the user clicking the icon or a menu item.
#[derive(Debug, Default)]
pub struct FakeTray {
    menu: Menu,
    tooltip: String,
    events: Vec<TrayEvent>,
}

impl FakeTray {
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently-set menu.
    pub fn menu(&self) -> &Menu {
        &self.menu
    }

    /// The currently-set tooltip.
    pub fn tooltip(&self) -> &str {
        &self.tooltip
    }

    /// Simulate a left-click on the tray icon.
    pub fn click_left(&mut self) {
        self.events.push(TrayEvent::LeftClick);
    }

    /// Simulate the user choosing menu item `id`. Returns `false` (and records
    /// nothing) if the item is absent or disabled.
    pub fn activate(&mut self, id: impl Into<MenuItemId>) -> bool {
        let id = id.into();
        if self.menu.can_activate(&id) {
            self.events.push(TrayEvent::MenuItem(id));
            true
        } else {
            false
        }
    }

    /// Drain the events recorded since the last drain.
    pub fn drain_events(&mut self) -> Vec<TrayEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Tray for FakeTray {
    fn set_menu(&mut self, menu: Menu) {
        self.menu = menu;
    }

    fn set_tooltip(&mut self, tooltip: &str) {
        self.tooltip = tooltip.to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jot_menu() -> Menu {
        Menu::new([
            MenuItem::action("capture", "Capture…"),
            MenuItem::action("agenda", "Agenda"),
            MenuItem::action("inbox", "Open inbox"),
            MenuItem::separator(),
            MenuItem::action("settings", "Settings"),
            MenuItem::action("quit", "Quit"),
        ])
    }

    #[test]
    fn set_menu_and_tooltip_are_recorded() {
        let mut tray = FakeTray::new();
        tray.set_menu(jot_menu());
        tray.set_tooltip("cockpit-jot");
        assert_eq!(tray.tooltip(), "cockpit-jot");
        assert_eq!(tray.menu().action_ids().count(), 5);
    }

    #[test]
    fn left_click_and_menu_activation_emit_events() {
        let mut tray = FakeTray::new();
        tray.set_menu(jot_menu());

        tray.click_left();
        assert!(tray.activate("agenda"));
        assert!(tray.activate("quit"));

        assert_eq!(
            tray.drain_events(),
            vec![
                TrayEvent::LeftClick,
                TrayEvent::MenuItem("agenda".into()),
                TrayEvent::MenuItem("quit".into()),
            ]
        );
        assert!(tray.drain_events().is_empty());
    }

    #[test]
    fn activating_unknown_item_is_ignored() {
        let mut tray = FakeTray::new();
        tray.set_menu(jot_menu());
        assert!(!tray.activate("nope"));
        assert!(tray.drain_events().is_empty());
    }
}
