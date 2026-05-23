//! Zellij launch planning.
//!
//! This module is pure command construction plus injectable binary lookup. The
//! PTY layer later consumes [`CommandSpec`] to spawn the selected command.

use std::{
    env,
    path::{Path, PathBuf},
};

/// A process command ready for the PTY layer to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    /// Create a command specification.
    pub fn new(program: impl Into<String>, args: impl Into<Vec<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into(),
        }
    }
}

/// Reason the launcher selected a plain-shell fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    MiseMissing,
    ZellijMissing,
}

/// Terminal launch plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchPlan {
    Zellij(CommandSpec),
    Fallback {
        command: CommandSpec,
        reason: FallbackReason,
    },
}

/// Platform-specific fallback shell profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellProfile {
    PowerShell,
    UnixShell,
}

impl ShellProfile {
    /// Host default shell profile.
    pub fn host_default() -> Self {
        if cfg!(windows) {
            Self::PowerShell
        } else {
            Self::UnixShell
        }
    }

    /// Command spec for this fallback profile.
    pub fn command(self) -> CommandSpec {
        match self {
            Self::PowerShell => CommandSpec::new("powershell.exe", Vec::<String>::new()),
            Self::UnixShell => CommandSpec::new(
                env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                Vec::<String>::new(),
            ),
        }
    }
}

/// A lookup for external binaries.
pub trait BinaryLookup {
    /// True if `binary` is available to launch.
    fn exists(&self, binary: &str) -> bool;
}

/// `PATH`-based binary lookup for production code.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathBinaryLookup;

impl BinaryLookup for PathBinaryLookup {
    fn exists(&self, binary: &str) -> bool {
        binary_in_path(binary)
    }
}

/// Build the Zellij command. When `layout` is set the command applies it to
/// the new session via the top-level `--layout <path>` flag (spec §10 v0.3);
/// Zellij only applies the layout the first time the session is created.
pub fn zellij_command(project_name: &str, layout: Option<&Path>) -> CommandSpec {
    let mut args = vec!["exec".to_string(), "--".to_string(), "zellij".to_string()];
    if let Some(layout) = layout {
        args.push("--layout".to_string());
        args.push(layout.display().to_string());
    }
    args.push("attach".to_string());
    args.push("--create".to_string());
    args.push(safe_session_name(project_name));

    CommandSpec::new("mise", args)
}

/// Select either the Zellij command or a plain-shell fallback based on binary
/// availability. When `layout` is set, a successful Zellij plan opens it.
pub fn plan_launch(
    project_name: &str,
    layout: Option<&Path>,
    lookup: &impl BinaryLookup,
    fallback: ShellProfile,
) -> LaunchPlan {
    if !lookup.exists("mise") {
        return LaunchPlan::Fallback {
            command: fallback.command(),
            reason: FallbackReason::MiseMissing,
        };
    }
    if !lookup.exists("zellij") {
        return LaunchPlan::Fallback {
            command: fallback.command(),
            reason: FallbackReason::ZellijMissing,
        };
    }

    LaunchPlan::Zellij(zellij_command(project_name, layout))
}

/// Convert an arbitrary project display name to a stable Zellij session name.
pub fn safe_session_name(project_name: &str) -> String {
    let mut out = String::new();
    let mut last_was_dash = false;

    for ch in project_name.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.') {
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "project".to_string()
    } else {
        trimmed
    }
}

fn binary_in_path(binary: &str) -> bool {
    let path = env::var_os("PATH").unwrap_or_default();
    env::split_paths(&path).any(|dir| executable_candidate_exists(&dir, binary))
}

fn executable_candidate_exists(dir: &Path, binary: &str) -> bool {
    if cfg!(windows) {
        let pathext = env::var_os("PATHEXT")
            .map(|value| {
                env::split_paths(&value)
                    .map(|path| path.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| vec![".exe".to_string(), ".cmd".to_string(), ".bat".to_string()]);
        pathext.iter().any(|ext| {
            let candidate = if binary
                .to_ascii_lowercase()
                .ends_with(&ext.to_ascii_lowercase())
            {
                PathBuf::from(binary)
            } else {
                PathBuf::from(format!("{binary}{ext}"))
            };
            dir.join(candidate).is_file()
        })
    } else {
        dir.join(binary).is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeLookup {
        mise: bool,
        zellij: bool,
    }

    impl BinaryLookup for FakeLookup {
        fn exists(&self, binary: &str) -> bool {
            match binary {
                "mise" => self.mise,
                "zellij" => self.zellij,
                _ => false,
            }
        }
    }

    #[test]
    fn sanitises_project_names_for_sessions() {
        assert_eq!(safe_session_name("Geotech Platform"), "geotech-platform");
        assert_eq!(safe_session_name("  AGS Tools  "), "ags-tools");
        assert_eq!(safe_session_name("qgis_plugin.v2"), "qgis_plugin.v2");
        assert_eq!(safe_session_name("***"), "project");
        assert_eq!(safe_session_name("a///b"), "a-b");
    }

    #[test]
    fn builds_mise_zellij_attach_command() {
        assert_eq!(
            zellij_command("Geotech Platform", None),
            CommandSpec::new(
                "mise",
                [
                    "exec",
                    "--",
                    "zellij",
                    "attach",
                    "--create",
                    "geotech-platform"
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
    }

    #[test]
    fn injects_layout_flag_when_a_layout_is_provided() {
        let layout = PathBuf::from("/projects/geotech/.config/zellij/dev.kdl");
        assert_eq!(
            zellij_command("Geotech Platform", Some(&layout)),
            CommandSpec::new(
                "mise",
                [
                    "exec",
                    "--",
                    "zellij",
                    "--layout",
                    "/projects/geotech/.config/zellij/dev.kdl",
                    "attach",
                    "--create",
                    "geotech-platform",
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
    }

    #[test]
    fn plans_zellij_when_binaries_exist() {
        let plan = plan_launch(
            "Project",
            None,
            &FakeLookup {
                mise: true,
                zellij: true,
            },
            ShellProfile::UnixShell,
        );
        assert_eq!(plan, LaunchPlan::Zellij(zellij_command("Project", None)));
    }

    #[test]
    fn plans_zellij_with_layout_when_one_is_given() {
        let layout = PathBuf::from("/tmp/dev.kdl");
        let plan = plan_launch(
            "Project",
            Some(&layout),
            &FakeLookup {
                mise: true,
                zellij: true,
            },
            ShellProfile::UnixShell,
        );
        assert_eq!(
            plan,
            LaunchPlan::Zellij(zellij_command("Project", Some(&layout)))
        );
    }

    #[test]
    fn falls_back_when_mise_is_missing() {
        let plan = plan_launch(
            "Project",
            None,
            &FakeLookup {
                mise: false,
                zellij: true,
            },
            ShellProfile::UnixShell,
        );
        assert_eq!(
            plan,
            LaunchPlan::Fallback {
                command: CommandSpec::new(
                    env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                    Vec::<String>::new()
                ),
                reason: FallbackReason::MiseMissing,
            }
        );
    }

    #[test]
    fn falls_back_when_zellij_is_missing() {
        let plan = plan_launch(
            "Project",
            None,
            &FakeLookup {
                mise: true,
                zellij: false,
            },
            ShellProfile::PowerShell,
        );
        assert_eq!(
            plan,
            LaunchPlan::Fallback {
                command: CommandSpec::new("powershell.exe", Vec::<String>::new()),
                reason: FallbackReason::ZellijMissing,
            }
        );
    }
}
