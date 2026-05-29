//! `launcher.toml` schema for the v0.13 quick-action launcher (M13.6).
//!
//! Parsed config for the `cockpit-quick` sibling binary, loaded from
//! `~/.config/cockpit/launcher.toml`. Pure data + parsing, mirroring the
//! [`Config`](crate::Config) pattern: `#[serde(default, deny_unknown_fields)]`
//! so a partial file fills defaults and a typo is a hard error rather than a
//! silently-ignored key.
//!
//! The schema is the one documented in `IMPLEMENTATION_PLAN.md` §8i M13.6:
//!
//! ```toml
//! [hotkey]
//! chord = "Ctrl+Space"
//!
//! [providers]
//! mise     = true
//! lua      = true
//! builtins = true
//!
//! [mise.projects]
//! paths = ["~/code/work", "~/code/personal"]
//!
//! [launcher.ui]
//! max_rows = 8
//! position = "centred"   # centred | top
//! theme    = "inherit"
//! ```

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Top-level `launcher.toml` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherConfig {
    pub hotkey: HotkeyConfig,
    pub providers: ProvidersConfig,
    pub mise: LauncherMiseConfig,
    pub launcher: LauncherSection,
}

impl LauncherConfig {
    /// Parse from a TOML string.
    pub fn from_toml(input: &str) -> Result<Self, ConfigError> {
        toml::from_str(input).map_err(ConfigError::Parse)
    }

    /// Load from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let input = fs::read_to_string(path).map_err(ConfigError::Read)?;
        Self::from_toml(&input)
    }

    /// Load from a file, returning defaults when the file is absent. A
    /// malformed file is still an error (matches the org/jot loader policy).
    pub fn load_optional(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(input) => Self::from_toml(&input),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(ConfigError::Read(err)),
        }
    }
}

/// Resolve the default location of `launcher.toml` on this OS, alongside the
/// main `config.toml`.
pub fn launcher_config_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "CodingCockpit", "cockpit")
        .map(|dirs| dirs.config_dir().join("launcher.toml"))
}

/// `[hotkey]` — the global summon chord.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HotkeyConfig {
    pub chord: String,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            chord: "Ctrl+Space".to_string(),
        }
    }
}

/// `[providers]` — per-provider enable flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProvidersConfig {
    pub mise: bool,
    pub lua: bool,
    pub builtins: bool,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            mise: true,
            lua: true,
            builtins: true,
        }
    }
}

/// `[mise]` — the mise-provider configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherMiseConfig {
    pub projects: MiseProjects,
}

/// `[mise.projects]` — the explicit project list. No filesystem crawl: every
/// mise project the launcher knows about is named here.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MiseProjects {
    /// Project root paths. A leading `~` is expanded by the binary loader.
    pub paths: Vec<String>,
}

/// `[launcher]` — UI-shaped settings, nested to match the documented schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherSection {
    pub ui: LauncherUi,
}

/// `[launcher.ui]` — popover presentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LauncherUi {
    /// Maximum rows shown in the results list.
    pub max_rows: usize,
    /// Where the popover anchors on screen.
    pub position: LauncherPosition,
    /// Theme name, or `inherit` to follow the main cockpit theme.
    pub theme: String,
}

impl Default for LauncherUi {
    fn default() -> Self {
        Self {
            max_rows: 8,
            position: LauncherPosition::default(),
            theme: "inherit".to_string(),
        }
    }
}

/// Popover anchor position.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LauncherPosition {
    /// Centred on the primary display (default).
    #[default]
    Centred,
    /// Anchored to the top of the primary display (Spotlight-style).
    Top,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_sample() {
        let cfg = LauncherConfig::from_toml(
            r#"
[hotkey]
chord = "Ctrl+Space"

[providers]
mise     = true
lua      = true
builtins = true

[mise.projects]
paths = ["~/code/work", "~/code/personal"]

[launcher.ui]
max_rows = 8
position = "centred"
theme    = "inherit"
"#,
        )
        .unwrap();
        // Every field but the project paths matches the defaults.
        assert_eq!(cfg.hotkey, HotkeyConfig::default());
        assert_eq!(cfg.providers, ProvidersConfig::default());
        assert_eq!(cfg.launcher, LauncherSection::default());
        assert_eq!(cfg.mise.projects.paths, ["~/code/work", "~/code/personal"]);
    }

    #[test]
    fn partial_config_fills_defaults() {
        let cfg = LauncherConfig::from_toml(
            r#"
[hotkey]
chord = "Ctrl+Alt+Space"

[launcher.ui]
position = "top"
"#,
        )
        .unwrap();
        assert_eq!(cfg.hotkey.chord, "Ctrl+Alt+Space");
        assert_eq!(cfg.launcher.ui.position, LauncherPosition::Top);
        // Untouched fields keep their defaults.
        assert_eq!(cfg.launcher.ui.max_rows, 8);
        assert!(cfg.providers.lua);
        assert!(cfg.mise.projects.paths.is_empty());
    }

    #[test]
    fn empty_document_is_all_defaults() {
        assert_eq!(
            LauncherConfig::from_toml("").unwrap(),
            LauncherConfig::default()
        );
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = LauncherConfig::from_toml("[providers]\nspotify = true\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn unknown_position_is_an_error() {
        let err = LauncherConfig::from_toml("[launcher.ui]\nposition = \"left\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn absent_file_returns_defaults() {
        let path = std::env::temp_dir().join(format!(
            "cockpit-missing-launcher-{}.toml",
            std::process::id()
        ));
        assert_eq!(
            LauncherConfig::load_optional(path).unwrap(),
            LauncherConfig::default()
        );
    }
}
