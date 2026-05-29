//! The tray menu model.
//!
//! Deliberately flat — the plan keeps the tray menu to a handful of items with
//! no nested submenus (fewer `tray-icon` quirks per OS). A menu is an ordered
//! list of [`MenuItem`]s and separators.

/// Stable identifier for a clickable menu item (e.g. `"capture"`, `"quit"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MenuItemId(pub String);

impl<S: Into<String>> From<S> for MenuItemId {
    fn from(s: S) -> Self {
        MenuItemId(s.into())
    }
}

/// One entry in the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// A clickable item.
    Action {
        id: MenuItemId,
        label: String,
        enabled: bool,
    },
    /// A non-interactive divider.
    Separator,
}

impl MenuItem {
    /// An enabled action item.
    pub fn action(id: impl Into<MenuItemId>, label: impl Into<String>) -> Self {
        MenuItem::Action {
            id: id.into(),
            label: label.into(),
            enabled: true,
        }
    }

    /// A divider.
    pub fn separator() -> Self {
        MenuItem::Separator
    }

    /// Mark an action disabled (no-op on a separator).
    pub fn disabled(mut self) -> Self {
        if let MenuItem::Action { enabled, .. } = &mut self {
            *enabled = false;
        }
        self
    }

    /// This item's id, if it is an action.
    pub fn id(&self) -> Option<&MenuItemId> {
        match self {
            MenuItem::Action { id, .. } => Some(id),
            MenuItem::Separator => None,
        }
    }

    fn is_enabled_action_with(&self, target: &MenuItemId) -> bool {
        matches!(
            self,
            MenuItem::Action { id, enabled: true, .. } if id == target
        )
    }
}

/// An ordered tray menu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Menu {
    pub items: Vec<MenuItem>,
}

impl Menu {
    /// Build a menu from items.
    pub fn new(items: impl IntoIterator<Item = MenuItem>) -> Self {
        Menu {
            items: items.into_iter().collect(),
        }
    }

    /// `true` if `id` names an enabled action in this menu.
    pub fn can_activate(&self, id: &MenuItemId) -> bool {
        self.items.iter().any(|it| it.is_enabled_action_with(id))
    }

    /// All action ids in order (skipping separators).
    pub fn action_ids(&self) -> impl Iterator<Item = &MenuItemId> {
        self.items.iter().filter_map(MenuItem::id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_activate_only_enabled_actions() {
        let menu = Menu::new([
            MenuItem::action("capture", "Capture…"),
            MenuItem::separator(),
            MenuItem::action("settings", "Settings").disabled(),
        ]);
        assert!(menu.can_activate(&"capture".into()));
        assert!(!menu.can_activate(&"settings".into())); // disabled
        assert!(!menu.can_activate(&"missing".into()));
    }

    #[test]
    fn action_ids_skip_separators() {
        let menu = Menu::new([
            MenuItem::action("a", "A"),
            MenuItem::separator(),
            MenuItem::action("b", "B"),
        ]);
        let ids: Vec<_> = menu.action_ids().map(|i| i.0.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
