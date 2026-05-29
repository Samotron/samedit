//! The Lua-extension provider (M13.3, launcher side).
//!
//! v0.9's `cockpit-lua` gains a `cockpit.launcher.action { id, title, run }`
//! namespace; the sibling binary loads `~/.config/cockpit/extensions/*.lua`
//! and collects the registered actions. This provider turns those
//! registrations into launcher entries. Each emits an [`ActionRun::Lua`]
//! handle keyed by `(extension, id)`; the binary routes Enter back to the
//! owning Lua VM to run the action's closure (with the same capability gate
//! and sandbox as every other extension call).
//!
//! Keeping the provider over plain data — a list of [`LuaAction`] descriptors
//! — means `cockpit-launcher` does **not** depend on `cockpit-lua`: the binary
//! bridges the two. The provider stays headless and unit-testable, and the
//! launcher core has no Lua VM in it.

use crate::action::{Action, ActionIcon, ActionRun, LuaActionHandle};
use crate::provider::ActionProvider;

/// A registered Lua launcher action, as the binary harvests it from a loaded
/// extension's `cockpit.launcher.action {…}` calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaAction {
    /// Extension file stem that registered the action.
    pub extension: String,
    /// Action id from `cockpit.launcher.action { id = … }`.
    pub id: String,
    /// Human title shown (and matched) in the launcher.
    pub title: String,
}

impl LuaAction {
    /// Construct a descriptor.
    pub fn new(
        extension: impl Into<String>,
        id: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            extension: extension.into(),
            id: id.into(),
            title: title.into(),
        }
    }
}

/// Provider over the launcher actions registered by Lua extensions.
#[derive(Debug, Default, Clone)]
pub struct LuaActionsProvider {
    actions: Vec<LuaAction>,
}

impl LuaActionsProvider {
    /// Build from the harvested descriptors.
    pub fn new(actions: impl IntoIterator<Item = LuaAction>) -> Self {
        Self {
            actions: actions.into_iter().collect(),
        }
    }

    /// Number of registered actions.
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// True when no extension registered a launcher action.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

impl ActionProvider for LuaActionsProvider {
    fn id(&self) -> &str {
        "lua"
    }

    fn search(&self, _query: &str) -> Vec<Action> {
        self.actions
            .iter()
            .map(|a| {
                Action::new(
                    format!("lua:{}:{}", a.extension, a.id),
                    a.title.clone(),
                    ActionRun::Lua(LuaActionHandle::new(&a.extension, &a.id)),
                )
                .with_subtitle(a.extension.clone())
                .with_icon(ActionIcon::Lua)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_lua_actions_with_handles() {
        let provider = LuaActionsProvider::new([
            LuaAction::new("weather", "user.weather", "Today's weather"),
            LuaAction::new("tools", "user.uuid", "Generate UUID"),
        ]);
        assert_eq!(provider.len(), 2);

        let actions = provider.search("");
        assert_eq!(actions[0].title, "Today's weather");
        assert!(matches!(
            &actions[0].run,
            ActionRun::Lua(h) if h.extension == "weather" && h.id == "user.weather"
        ));
        // The id is namespaced so two extensions can share an action id.
        assert_eq!(actions[0].id, "lua:weather:user.weather");
    }

    #[test]
    fn empty_provider_emits_nothing() {
        let provider = LuaActionsProvider::default();
        assert!(provider.is_empty());
        assert!(provider.search("anything").is_empty());
    }
}
