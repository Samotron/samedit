//! Leader ("which-key") menu view-model (spec §16 — discoverable keybinds).
//!
//! Headless model of a Doom-Emacs / Spacemacs style leader menu: the user
//! presses the configured leader key (default `Space`) and a popup shows the
//! keys available at the current level, each labelled with the group or
//! command it triggers. Pressing a key either drills into a sub-group or
//! returns a [`CommandId`] for the caller to dispatch through the single
//! command spine.
//!
//! The menu is a pure tree navigator over [`LeaderItem`] nodes. All rendering
//! lives in the binary; this module only tracks which level is showing and
//! resolves key presses, so the behaviour is fully unit-testable without a
//! window.

use cockpit_commands::CommandId;

/// What a leader key maps to: a nested group of further keys, or a terminal
/// command id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderKind {
    /// A sub-menu reached by pressing this item's key.
    Group(Vec<LeaderItem>),
    /// A command dispatched when this item's key is pressed.
    Command(CommandId),
}

/// One selectable row in a leader-menu level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderItem {
    /// The single key the user presses to choose this row (e.g. `"f"`).
    pub key: String,
    /// Human-readable label shown next to the key (e.g. `"File…"` / `"Save"`).
    pub label: String,
    /// Whether this row opens a sub-group or runs a command.
    pub kind: LeaderKind,
}

impl LeaderItem {
    /// A row that dispatches `id` when its `key` is pressed.
    pub fn command(
        key: impl Into<String>,
        label: impl Into<String>,
        id: impl Into<CommandId>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: LeaderKind::Command(id.into()),
        }
    }

    /// A row that opens a sub-group of further rows when its `key` is pressed.
    pub fn group(
        key: impl Into<String>,
        label: impl Into<String>,
        children: Vec<LeaderItem>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: LeaderKind::Group(children),
        }
    }

    /// True when pressing this row drills into a sub-menu.
    pub fn is_group(&self) -> bool {
        matches!(self.kind, LeaderKind::Group(_))
    }
}

/// Result of pressing a key while the menu is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaderOutcome {
    /// The key opened a sub-group; the menu is still open one level deeper.
    Descended,
    /// The key resolved to a command id; the caller should dispatch it and
    /// close the menu.
    Run(CommandId),
    /// No row at the current level uses that key; the caller closes the menu.
    Unknown,
}

/// Leader-menu view-model: a stack of levels with the current level on top.
#[derive(Debug, Clone)]
pub struct LeaderMenu {
    /// Each entry is one displayed level; `stack[0]` is the root. Never empty.
    stack: Vec<Vec<LeaderItem>>,
    /// Keys pressed to reach the current level (the breadcrumb trail).
    breadcrumb: Vec<String>,
}

impl LeaderMenu {
    /// Build a menu rooted at `root`. Every level is sorted by key so the
    /// which-key display is deterministic regardless of construction order.
    pub fn new(mut root: Vec<LeaderItem>) -> Self {
        sort_level(&mut root);
        Self {
            stack: vec![root],
            breadcrumb: Vec::new(),
        }
    }

    /// Rows shown at the current level, sorted by key.
    pub fn items(&self) -> &[LeaderItem] {
        // `stack` is never empty by construction.
        self.stack.last().map(Vec::as_slice).unwrap_or(&[])
    }

    /// Keys pressed to reach the current level, outermost first.
    pub fn breadcrumb(&self) -> &[String] {
        &self.breadcrumb
    }

    /// How many groups deep the menu currently is (0 at the root).
    pub fn depth(&self) -> usize {
        self.stack.len() - 1
    }

    /// True while the root level is showing.
    pub fn at_root(&self) -> bool {
        self.depth() == 0
    }

    /// Press `key` at the current level.
    ///
    /// Returns [`LeaderOutcome::Descended`] (and pushes the sub-level) when the
    /// key opens a group, [`LeaderOutcome::Run`] when it maps to a command, and
    /// [`LeaderOutcome::Unknown`] when no row uses that key.
    pub fn press(&mut self, key: &str) -> LeaderOutcome {
        let Some(item) = self.items().iter().find(|item| item.key == key).cloned() else {
            return LeaderOutcome::Unknown;
        };
        match item.kind {
            LeaderKind::Command(id) => LeaderOutcome::Run(id),
            LeaderKind::Group(children) => {
                // Children were sorted when the tree was built.
                self.stack.push(children);
                self.breadcrumb.push(item.key);
                LeaderOutcome::Descended
            }
        }
    }

    /// Pop one level. Returns `false` (without changing state) when already at
    /// the root, so the caller can close the menu instead.
    pub fn back(&mut self) -> bool {
        if self.at_root() {
            return false;
        }
        self.stack.pop();
        self.breadcrumb.pop();
        true
    }
}

/// Sort one level by key and recurse into groups so the whole tree is ordered.
fn sort_level(items: &mut [LeaderItem]) {
    items.sort_by(|a, b| a.key.cmp(&b.key));
    for item in items.iter_mut() {
        if let LeaderKind::Group(children) = &mut item.kind {
            sort_level(children);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> LeaderMenu {
        LeaderMenu::new(vec![
            LeaderItem::command("q", "Quit", "app.quit"),
            LeaderItem::group(
                "f",
                "File…",
                vec![
                    LeaderItem::command("s", "Save", "file.save"),
                    LeaderItem::command("o", "Open", "file.open"),
                ],
            ),
        ])
    }

    #[test]
    fn root_level_is_sorted_by_key() {
        let menu = fixture();
        let keys: Vec<&str> = menu.items().iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["f", "q"]);
        assert!(menu.at_root());
        assert_eq!(menu.depth(), 0);
    }

    #[test]
    fn pressing_a_command_key_returns_its_id() {
        let mut menu = fixture();
        assert_eq!(
            menu.press("q"),
            LeaderOutcome::Run(CommandId::from("app.quit"))
        );
    }

    #[test]
    fn pressing_a_group_key_descends_and_tracks_breadcrumb() {
        let mut menu = fixture();
        assert_eq!(menu.press("f"), LeaderOutcome::Descended);
        assert_eq!(menu.depth(), 1);
        assert_eq!(menu.breadcrumb(), &["f".to_string()]);

        let keys: Vec<&str> = menu.items().iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["o", "s"]);

        assert_eq!(
            menu.press("s"),
            LeaderOutcome::Run(CommandId::from("file.save"))
        );
    }

    #[test]
    fn unknown_key_does_not_change_level() {
        let mut menu = fixture();
        assert_eq!(menu.press("z"), LeaderOutcome::Unknown);
        assert!(menu.at_root());
    }

    #[test]
    fn back_pops_one_level_and_refuses_at_root() {
        let mut menu = fixture();
        assert!(!menu.back(), "root has nothing to pop");

        menu.press("f");
        assert_eq!(menu.depth(), 1);
        assert!(menu.back());
        assert!(menu.at_root());
        assert!(menu.breadcrumb().is_empty());
    }

    #[test]
    fn nested_groups_are_sorted_recursively() {
        let menu = LeaderMenu::new(vec![LeaderItem::group(
            "a",
            "AI…",
            vec![
                LeaderItem::command("c", "Codex", "tool.codex"),
                LeaderItem::command("a", "Claude", "tool.claude"),
            ],
        )]);
        let mut menu = menu;
        menu.press("a");
        let keys: Vec<&str> = menu.items().iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "c"]);
    }
}
