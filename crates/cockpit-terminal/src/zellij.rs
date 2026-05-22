//! Zellij launch planning.
//!
//! This module is pure command construction plus injectable binary lookup. The
//! PTY layer later consumes [`CommandSpec`] to spawn the selected command.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

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

/// A project-specific Zellij layout that has been parsed as KDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijLayout {
    path: PathBuf,
}

impl ZellijLayout {
    /// Read a layout file and validate that it is well-formed KDL.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, ZellijLayoutError> {
        let path = path.into();
        let input = fs::read_to_string(&path).map_err(ZellijLayoutError::Read)?;
        input
            .parse::<kdl::KdlDocument>()
            .map_err(ZellijLayoutError::Parse)?;
        Ok(Self { path })
    }

    /// Path to the validated layout file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Zellij layout validation error.
#[derive(Debug, Error)]
pub enum ZellijLayoutError {
    #[error("failed to read Zellij layout: {0}")]
    Read(#[source] io::Error),
    #[error("failed to parse Zellij layout KDL: {0}")]
    Parse(#[source] kdl::KdlError),
}

/// `PATH`-based binary lookup for production code.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathBinaryLookup;

impl BinaryLookup for PathBinaryLookup {
    fn exists(&self, binary: &str) -> bool {
        binary_in_path(binary)
    }
}

/// Build the Zellij command for a project.
pub fn zellij_command(project_name: &str, layout: Option<&ZellijLayout>) -> CommandSpec {
    let session = safe_session_name(project_name);
    let zellij_args = if let Some(layout) = layout {
        vec![
            "zellij".to_string(),
            "--layout".to_string(),
            layout.path().display().to_string(),
            "--session".to_string(),
            session,
        ]
    } else {
        vec![
            "zellij".to_string(),
            "attach".to_string(),
            "--create".to_string(),
            session,
        ]
    };

    CommandSpec::new(
        "mise",
        ["exec".to_string(), "--".to_string()]
            .into_iter()
            .chain(zellij_args)
            .collect::<Vec<_>>(),
    )
}

/// Build the v0.1 Zellij attach command.
pub fn zellij_attach_command(project_name: &str) -> CommandSpec {
    CommandSpec::new(
        "mise",
        vec![
            "exec".to_string(),
            "--".to_string(),
            "zellij".to_string(),
            "attach".to_string(),
            "--create".to_string(),
            safe_session_name(project_name),
        ],
    )
}

/// Select either the Zellij command or a plain-shell fallback based on binary
/// availability.
pub fn plan_launch(
    project_name: &str,
    layout: Option<&ZellijLayout>,
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
    fn validates_layout_kdl_and_builds_layout_command() {
        let tempdir = tempfile::tempdir().unwrap();
        let layout_path = tempdir.path().join("dev.kdl");
        fs::write(&layout_path, "layout { pane }\n").unwrap();

        let layout = ZellijLayout::load(&layout_path).unwrap();

        assert_eq!(
            zellij_command("Geotech Platform", Some(&layout)),
            CommandSpec::new(
                "mise",
                [
                    "exec",
                    "--",
                    "zellij",
                    "--layout",
                    layout_path.to_str().unwrap(),
                    "--session",
                    "geotech-platform"
                ]
                .map(str::to_string)
                .to_vec()
            )
        );
    }

    #[test]
    fn rejects_invalid_layout_kdl() {
        let tempdir = tempfile::tempdir().unwrap();
        let layout_path = tempdir.path().join("bad.kdl");
        fs::write(&layout_path, "layout {").unwrap();

        assert!(matches!(
            ZellijLayout::load(&layout_path),
            Err(ZellijLayoutError::Parse(_))
        ));
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
