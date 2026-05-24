//! Project launcher application — Track D wire-up (M1.13).
//!
//! [`LauncherModel`] is the project selection screen state. It uses the
//! headless [`Launcher`](cockpit_ui::launcher::Launcher) from `cockpit-ui` and
//! dispatches its intents back to the binary to open a real project workspace.
//! The [`CockpitApp`] trait is implemented for `LauncherModel` directly so it
//! can sit inside the [`AppShell`](crate::app::AppShell) state machine (M7.1).
//! After the launcher signals a selection, the shell transitions to hydrating
//! the chosen project — all within the same `winit` event loop.

use std::path::PathBuf;

use cockpit_commands::KeyChord;
use cockpit_render::{CockpitApp, Painter, Theme, Viewport};
use cockpit_ui::launcher::{Launcher, LauncherAction, LauncherIntent, LauncherSelection};

use crate::app::{CHAR_W_RATIO, FONT, PAD, ROW_H};

/// Result of the launcher's event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherResult {
    /// Open the project at this path.
    OpenProject(PathBuf),
    /// The user requested to quit the launcher.
    Exit,
}

/// The project launcher view-model.
pub struct LauncherModel {
    launcher: Launcher,
    theme: Theme,
    result: Option<LauncherResult>,
}

impl LauncherModel {
    /// Build a launcher model from recent projects.
    pub fn new(recents: Vec<cockpit_ui::launcher::RecentProject>) -> Self {
        Self {
            launcher: Launcher::new(recents),
            theme: Theme::default(),
            result: None,
        }
    }

    /// The result of the launcher session, if any.
    pub fn result(&self) -> Option<LauncherResult> {
        self.result.clone()
    }

    fn handle_intent(&mut self, intent: LauncherIntent) {
        match intent {
            LauncherIntent::OpenRecent(index) => {
                if let Some(project) = self.launcher.recents().get(index) {
                    self.result = Some(LauncherResult::OpenProject(project.root_path.clone()));
                }
            }
            LauncherIntent::Action(action) => match action {
                LauncherAction::OpenFolder => {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.result = Some(LauncherResult::OpenProject(path));
                    }
                }
                LauncherAction::CloneFromGit => {
                    // v0.1: No-op.
                }
                LauncherAction::NewProject => {
                    // v0.1: No-op.
                }
            },
        }
    }
}

impl CockpitApp for LauncherModel {
    fn paint(&mut self, painter: &mut Painter, viewport: Viewport) {
        let scale = viewport.scale.max(0.5);
        let width = viewport.width as f32 / scale;
        let height = viewport.height as f32 / scale;

        // Clear background.
        painter.rect(
            cockpit_render::Rect::new(0.0, 0.0, viewport.width as f32, viewport.height as f32),
            self.theme.background,
        );

        let panel_w = 600.0_f32.min(width - 2.0 * PAD);
        let panel_h = 400.0_f32.min(height - 2.0 * PAD);
        let panel_x = (width - panel_w) / 2.0;
        let panel_y = (height - panel_h) / 2.0;

        // Panel frame.
        painter.rect(
            cockpit_render::Rect::new(
                panel_x * scale,
                panel_y * scale,
                panel_w * scale,
                panel_h * scale,
            ),
            self.theme.pane_background,
        );
        painter.rect(
            cockpit_render::Rect::new(
                panel_x * scale,
                panel_y * scale,
                panel_w * scale,
                2.0 * scale,
            ),
            self.theme.accent,
        );

        // Title.
        painter.text(
            (panel_x + PAD) * scale,
            (panel_y + PAD) * scale,
            "Coding Cockpit",
            self.theme.text,
            24.0 * scale,
        );

        // Recents section.
        let recents_y = panel_y + 60.0;
        painter.text(
            (panel_x + PAD) * scale,
            recents_y * scale,
            "Recent Projects",
            self.theme.muted_text,
            FONT * scale,
        );

        let mut current_y = recents_y + 30.0;
        if self.launcher.recents().is_empty() {
            painter.text(
                (panel_x + PAD * 2.0) * scale,
                current_y * scale,
                "No recent projects found.",
                self.theme.muted_text,
                FONT * scale,
            );
        } else {
            for (i, project) in self.launcher.recents().iter().enumerate() {
                let selected =
                    matches!(self.launcher.selection(), LauncherSelection::Recent(idx) if idx == i);
                if selected {
                    painter.rect(
                        cockpit_render::Rect::new(
                            (panel_x + 2.0) * scale,
                            current_y * scale,
                            (panel_w - 4.0) * scale,
                            ROW_H * scale,
                        ),
                        self.theme.selection,
                    );
                }
                painter.text(
                    (panel_x + PAD * 2.0) * scale,
                    (current_y + 3.0) * scale,
                    &project.display_name,
                    if selected {
                        self.theme.text
                    } else {
                        self.theme.muted_text
                    },
                    FONT * scale,
                );
                let path_text = project.root_path.display().to_string();
                let char_w = FONT * CHAR_W_RATIO;
                painter.text(
                    (panel_x + panel_w - PAD - path_text.chars().count() as f32 * char_w) * scale,
                    (current_y + 3.0) * scale,
                    path_text,
                    self.theme.muted_text,
                    (FONT - 2.0) * scale,
                );
                current_y += ROW_H;
            }
        }

        // Actions section (footer).
        let actions_y = panel_y + panel_h - 60.0;
        let mut action_x = panel_x + PAD;
        for action in self.launcher.actions() {
            let selected =
                matches!(self.launcher.selection(), LauncherSelection::Action(a) if a == action);
            let label = action.label();
            let label_w = label.chars().count() as f32 * FONT * CHAR_W_RATIO + 20.0;

            if selected {
                painter.rect(
                    cockpit_render::Rect::new(
                        action_x * scale,
                        actions_y * scale,
                        label_w * scale,
                        30.0 * scale,
                    ),
                    self.theme.selection,
                );
                painter.rect(
                    cockpit_render::Rect::new(
                        action_x * scale,
                        (actions_y + 28.0) * scale,
                        label_w * scale,
                        2.0 * scale,
                    ),
                    self.theme.accent,
                );
            }

            painter.text(
                (action_x + 10.0) * scale,
                (actions_y + 8.0) * scale,
                label,
                if selected {
                    self.theme.text
                } else {
                    self.theme.muted_text
                },
                FONT * scale,
            );
            action_x += label_w + PAD;
        }

        // Help text.
        painter.text(
            (panel_x + PAD) * scale,
            (panel_y + panel_h - 20.0) * scale,
            "Arrows to navigate, Enter to select, Esc to quit.",
            self.theme.muted_text,
            (FONT - 2.0) * scale,
        );
    }

    fn theme(&self) -> &Theme {
        &self.theme
    }

    fn on_key(&mut self, chord: KeyChord) {
        let stroke = chord.strokes().first().unwrap();
        let key = stroke.key();
        let modifiers = stroke.modifiers();

        if key == "Escape" && modifiers.is_none() {
            self.result = Some(LauncherResult::Exit);
            return;
        }

        if key == "Enter" && modifiers.is_none() {
            let intent = self.launcher.activate();
            self.handle_intent(intent);
            return;
        }

        match key {
            "ArrowDown" | "j" if modifiers.is_none() => self.launcher.move_down(),
            "ArrowUp" | "k" if modifiers.is_none() => self.launcher.move_up(),
            "ArrowRight" | "l" if modifiers.is_none() => {
                if let LauncherSelection::Action(_) = self.launcher.selection() {
                    self.launcher.move_down();
                }
            }
            "ArrowLeft" | "h" if modifiers.is_none() => {
                if let LauncherSelection::Action(_) = self.launcher.selection() {
                    self.launcher.move_up();
                }
            }
            _ => {}
        }
    }

    fn wants_exit(&self) -> bool {
        self.result.is_some()
    }
}
