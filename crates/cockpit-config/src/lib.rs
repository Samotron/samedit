//! `cockpit-config` — typed user and project configuration.
//!
//! Loads the user config (spec §20) from TOML, and later Zellij layout files
//! from KDL. Pure data + parsing; no I/O side effects beyond reading files.

use std::{collections::BTreeMap, fs, io, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level user configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: UiConfig,
    pub editor: EditorConfig,
    pub project: ProjectConfig,
    pub mise: MiseConfig,
    pub terminal: TerminalConfig,
    pub keys: KeysConfig,
}

impl Config {
    /// Parse config from a TOML string.
    pub fn from_toml(input: &str) -> Result<Self, ConfigError> {
        toml::from_str(input).map_err(ConfigError::Parse)
    }

    /// Load config from a file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let input = fs::read_to_string(path).map_err(ConfigError::Read)?;
        Self::from_toml(&input)
    }

    /// Load config from a file, returning defaults if the file is absent.
    pub fn load_optional(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(input) => Self::from_toml(&input),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(ConfigError::Read(err)),
        }
    }
}

/// UI settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub theme: String,
    pub font: String,
    pub font_size: u16,
    pub left_width: u16,
    pub right_width: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            font: "JetBrains Mono".to_string(),
            font_size: 13,
            left_width: 260,
            right_width: 480,
        }
    }
}

/// Editor settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorConfig {
    pub vim_mode: bool,
    pub line_numbers: bool,
    pub relative_line_numbers: bool,
    pub tab_width: u8,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: true,
            line_numbers: true,
            relative_line_numbers: true,
            tab_width: 4,
        }
    }
}

/// Project settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectConfig {
    pub environment_provider: String,
    pub project_launcher: bool,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            environment_provider: "mise".to_string(),
            project_launcher: true,
        }
    }
}

/// Mise integration settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MiseConfig {
    pub enabled: bool,
    pub auto_detect: bool,
    pub auto_install: bool,
    pub use_for_terminal: bool,
    pub use_for_tasks: bool,
    pub use_for_lsp: bool,
}

impl Default for MiseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_detect: true,
            auto_install: false,
            use_for_terminal: true,
            use_for_tasks: true,
            use_for_lsp: true,
        }
    }
}

/// Terminal settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    pub engine: String,
    pub workspace: String,
    pub default_profile: String,
    pub profiles: BTreeMap<String, TerminalProfile>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            engine: "termwiz".to_string(),
            workspace: "zellij".to_string(),
            default_profile: "project-zellij".to_string(),
            profiles: BTreeMap::from([(
                "project-zellij".to_string(),
                TerminalProfile::project_zellij(),
            )]),
        }
    }
}

/// A named terminal profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalProfile {
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
}

impl TerminalProfile {
    fn project_zellij() -> Self {
        Self {
            label: "Project Zellij".to_string(),
            command: "mise".to_string(),
            args: vec![
                "exec".to_string(),
                "--".to_string(),
                "zellij".to_string(),
                "attach".to_string(),
                "--create".to_string(),
                "{project_name}".to_string(),
            ],
        }
    }
}

/// All configured key groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeysConfig {
    pub global: GlobalKeys,
}

/// Global keybindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GlobalKeys {
    pub focus_files: String,
    pub focus_editor: String,
    pub focus_terminal: String,
    pub toggle_terminal: String,
    pub toggle_files: String,
    pub command_palette: String,
    pub fuzzy_open: String,
}

impl Default for GlobalKeys {
    fn default() -> Self {
        Self {
            focus_files: "Ctrl+h".to_string(),
            focus_editor: "Ctrl+j".to_string(),
            focus_terminal: "Ctrl+l".to_string(),
            toggle_terminal: "Ctrl+`".to_string(),
            toggle_files: "Ctrl+b".to_string(),
            command_palette: "Ctrl+Shift+p".to_string(),
            fuzzy_open: "Ctrl+p".to_string(),
        }
    }
}

/// Config loading/parsing error.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config: {0}")]
    Read(#[source] io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[source] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_SAMPLE: &str = r#"
[ui]
theme = "dark"
font = "JetBrains Mono"
font_size = 13
left_width = 260
right_width = 480

[editor]
vim_mode = true
line_numbers = true
relative_line_numbers = true
tab_width = 4

[project]
environment_provider = "mise"
project_launcher = true

[mise]
enabled = true
auto_detect = true
auto_install = false
use_for_terminal = true
use_for_tasks = true
use_for_lsp = true

[terminal]
engine = "termwiz"
workspace = "zellij"
default_profile = "project-zellij"

[terminal.profiles.project-zellij]
label = "Project Zellij"
command = "mise"
args = ["exec", "--", "zellij", "attach", "--create", "{project_name}"]

[keys.global]
focus_files = "Ctrl+h"
focus_editor = "Ctrl+j"
focus_terminal = "Ctrl+l"
toggle_terminal = "Ctrl+`"
toggle_files = "Ctrl+b"
command_palette = "Ctrl+Shift+p"
fuzzy_open = "Ctrl+p"
"#;

    #[test]
    fn parses_spec_sample() {
        let config = Config::from_toml(SPEC_SAMPLE).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(
            config.terminal.profiles["project-zellij"].args,
            [
                "exec",
                "--",
                "zellij",
                "attach",
                "--create",
                "{project_name}"
            ]
        );
    }

    #[test]
    fn partial_config_fills_defaults() {
        let config = Config::from_toml(
            r#"
[ui]
font_size = 16

[keys.global]
command_palette = "Ctrl+Shift+P"
"#,
        )
        .unwrap();

        assert_eq!(config.ui.font_size, 16);
        assert_eq!(config.ui.left_width, 260);
        assert_eq!(config.keys.global.command_palette, "Ctrl+Shift+P");
        assert!(!config.mise.auto_install);
    }

    #[test]
    fn absent_optional_config_returns_defaults() {
        let path = std::env::temp_dir().join(format!(
            "cockpit-missing-config-{}.toml",
            std::process::id()
        ));
        let config = Config::load_optional(path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn malformed_toml_is_error() {
        let err = Config::from_toml("[ui\nfont_size = 13").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn unknown_fields_are_errors() {
        let err = Config::from_toml(
            r#"
[editor]
magic = true
"#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
}
