//! Project launcher view-model (spec §6 / §23 v0.1 M1.13).
//!
//! Pure data + selection logic for the launcher screen: the recent-projects
//! list cached on disk, the three primary actions (Open Folder / Clone from Git
//! / New Project), keyboard navigation, and a single [`LauncherIntent`] output
//! that the binary translates into real I/O. No filesystem or window access.

use std::path::PathBuf;

/// One recent project entry, as cached by `cockpit-project` (spec §7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentProject {
    pub display_name: String,
    pub root_path: PathBuf,
}

impl RecentProject {
    /// Construct a recent-project entry.
    pub fn new(display_name: impl Into<String>, root_path: impl Into<PathBuf>) -> Self {
        Self {
            display_name: display_name.into(),
            root_path: root_path.into(),
        }
    }
}

/// One of the launcher's footer actions (spec §6 bottom row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherAction {
    OpenFolder,
    CloneFromGit,
    NewProject,
}

impl LauncherAction {
    /// All actions, in display order.
    pub const ALL: [LauncherAction; 3] = [
        LauncherAction::OpenFolder,
        LauncherAction::CloneFromGit,
        LauncherAction::NewProject,
    ];

    /// Human-readable label shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenFolder => "Open Folder",
            Self::CloneFromGit => "Clone from Git",
            Self::NewProject => "New Project",
        }
    }
}

/// Which row of the launcher currently has the keyboard cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherSelection {
    /// A recent project by index into [`Launcher::recents`].
    Recent(usize),
    /// One of the footer actions.
    Action(LauncherAction),
}

/// Action requested by the user; the app shell turns this into real I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherIntent {
    /// Open the recent project at this index.
    OpenRecent(usize),
    /// Run one of the footer actions.
    Action(LauncherAction),
}

/// Project launcher view-model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    recents: Vec<RecentProject>,
    selection: LauncherSelection,
}

impl Launcher {
    /// Build a launcher from cached recent projects. Selection starts on the
    /// first recent project, or on Open Folder if there are none.
    pub fn new(recents: Vec<RecentProject>) -> Self {
        let selection = if recents.is_empty() {
            LauncherSelection::Action(LauncherAction::OpenFolder)
        } else {
            LauncherSelection::Recent(0)
        };
        Self { recents, selection }
    }

    /// Recent projects in display order (most recent first by convention).
    pub fn recents(&self) -> &[RecentProject] {
        &self.recents
    }

    /// Footer action rows.
    pub fn actions(&self) -> [LauncherAction; 3] {
        LauncherAction::ALL
    }

    /// Currently-highlighted row.
    pub fn selection(&self) -> LauncherSelection {
        self.selection
    }

    /// Move the selection down one row, wrapping at the bottom.
    pub fn move_down(&mut self) {
        self.selection = match self.selection {
            LauncherSelection::Recent(i) if i + 1 < self.recents.len() => {
                LauncherSelection::Recent(i + 1)
            }
            LauncherSelection::Recent(_) => LauncherSelection::Action(LauncherAction::OpenFolder),
            LauncherSelection::Action(action) => match action {
                LauncherAction::OpenFolder => {
                    LauncherSelection::Action(LauncherAction::CloneFromGit)
                }
                LauncherAction::CloneFromGit => {
                    LauncherSelection::Action(LauncherAction::NewProject)
                }
                LauncherAction::NewProject => {
                    if self.recents.is_empty() {
                        LauncherSelection::Action(LauncherAction::OpenFolder)
                    } else {
                        LauncherSelection::Recent(0)
                    }
                }
            },
        };
    }

    /// Move the selection up one row, wrapping at the top.
    pub fn move_up(&mut self) {
        self.selection = match self.selection {
            LauncherSelection::Recent(0) => LauncherSelection::Action(LauncherAction::NewProject),
            LauncherSelection::Recent(i) => LauncherSelection::Recent(i - 1),
            LauncherSelection::Action(LauncherAction::OpenFolder) => {
                if self.recents.is_empty() {
                    LauncherSelection::Action(LauncherAction::NewProject)
                } else {
                    LauncherSelection::Recent(self.recents.len() - 1)
                }
            }
            LauncherSelection::Action(LauncherAction::CloneFromGit) => {
                LauncherSelection::Action(LauncherAction::OpenFolder)
            }
            LauncherSelection::Action(LauncherAction::NewProject) => {
                LauncherSelection::Action(LauncherAction::CloneFromGit)
            }
        };
    }

    /// Directly select a recent project by index. No-op if out of range.
    pub fn select_recent(&mut self, index: usize) {
        if index < self.recents.len() {
            self.selection = LauncherSelection::Recent(index);
        }
    }

    /// Directly select a footer action.
    pub fn select_action(&mut self, action: LauncherAction) {
        self.selection = LauncherSelection::Action(action);
    }

    /// Activate the current selection.
    pub fn activate(&self) -> LauncherIntent {
        match self.selection {
            LauncherSelection::Recent(i) => LauncherIntent::OpenRecent(i),
            LauncherSelection::Action(action) => LauncherIntent::Action(action),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Launcher {
        Launcher::new(vec![
            RecentProject::new("geotech-platform", "/home/me/code/geotech"),
            RecentProject::new("ags-tools", "/home/me/code/ags-tools"),
            RecentProject::new("qgis-plugin", "/home/me/code/qgis-plugin"),
        ])
    }

    #[test]
    fn defaults_to_first_recent_when_recents_exist() {
        let launcher = fixture();
        assert_eq!(launcher.selection(), LauncherSelection::Recent(0));
        assert_eq!(launcher.recents().len(), 3);
    }

    #[test]
    fn defaults_to_open_folder_when_no_recents() {
        let launcher = Launcher::new(Vec::new());
        assert_eq!(
            launcher.selection(),
            LauncherSelection::Action(LauncherAction::OpenFolder)
        );
    }

    #[test]
    fn move_down_walks_recents_then_actions_then_wraps() {
        let mut launcher = fixture();
        launcher.move_down();
        assert_eq!(launcher.selection(), LauncherSelection::Recent(1));
        launcher.move_down();
        launcher.move_down();
        assert_eq!(
            launcher.selection(),
            LauncherSelection::Action(LauncherAction::OpenFolder)
        );
        launcher.move_down();
        launcher.move_down();
        assert_eq!(
            launcher.selection(),
            LauncherSelection::Action(LauncherAction::NewProject)
        );
        launcher.move_down();
        assert_eq!(launcher.selection(), LauncherSelection::Recent(0));
    }

    #[test]
    fn move_up_wraps_from_first_recent_to_last_action() {
        let mut launcher = fixture();
        launcher.move_up();
        assert_eq!(
            launcher.selection(),
            LauncherSelection::Action(LauncherAction::NewProject)
        );
    }

    #[test]
    fn activate_returns_intent_for_selection() {
        let mut launcher = fixture();
        assert_eq!(launcher.activate(), LauncherIntent::OpenRecent(0));

        launcher.select_action(LauncherAction::OpenFolder);
        assert_eq!(
            launcher.activate(),
            LauncherIntent::Action(LauncherAction::OpenFolder)
        );
    }

    #[test]
    fn select_recent_ignores_out_of_range_index() {
        let mut launcher = fixture();
        launcher.select_recent(42);
        assert_eq!(launcher.selection(), LauncherSelection::Recent(0));
    }
}
