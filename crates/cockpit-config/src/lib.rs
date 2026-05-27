//! `cockpit-config` — typed user and project configuration.
//!
//! Loads the user config (spec §20) from TOML and the native cockpit
//! multiplexer layout files (see [`cockpit_layout`]) introduced in v0.7
//! M7.8. Pure data + parsing; no I/O side effects beyond reading files.

pub mod cockpit_layout;

pub use cockpit_layout::{CockpitLayout, CockpitLayoutNode, CockpitSplitDirection};

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
    pub panes: PanesConfig,
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

/// Resolve the default location of the user config file on this OS —
/// `$XDG_CONFIG_HOME/cockpit/config.toml` on Linux,
/// `~/Library/Application Support/dev.CodingCockpit.cockpit/config.toml`
/// on macOS, `%APPDATA%\CodingCockpit\cockpit\config\config.toml` on
/// Windows. Returns `None` when the OS does not surface a config dir
/// (rare — typically headless CI).
pub fn user_config_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("dev", "CodingCockpit", "cockpit")
        .map(|dirs| dirs.config_dir().join("config.toml"))
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
    /// Run the project's `format` mise task (or LSP `textDocument/formatting`
    /// when no task is configured) after every successful save (M4.4).
    /// Default off so existing projects do not change behaviour until the
    /// user opts in.
    pub format_on_save: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: true,
            line_numbers: true,
            relative_line_numbers: true,
            tab_width: 4,
            format_on_save: false,
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

/// `[panes.tools.*]` config block — tool-pane recipes (v0.8 M8.2).
///
/// Each entry creates a keybindable command that opens an upstream CLI
/// (lazygit, claude, codex, …) in a multiplexer pane. The recipe is data
/// only — the binary turns it into a `CommandId` and the mux's
/// floating / docked pane primitives handle layout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PanesConfig {
    pub tools: std::collections::BTreeMap<String, ToolPaneRecipe>,
}

/// Where a tool-pane recipe places its pane (v0.8 M8.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolPaneLayout {
    /// Centred overlay above the regular layout (~80% × 80%).
    #[default]
    Floating,
    /// Full-height pane on the right of the workspace.
    SideRight,
    /// Full-width strip across the bottom of the workspace.
    Bottom,
}

/// One tool-pane recipe (v0.8 M8.2). The `name` field is set by the loader
/// from the table key — the TOML schema does not duplicate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPaneRecipe {
    /// Shell command line spawned in the pane.
    pub command: String,
    /// Layout slot. Defaults to `floating`.
    pub layout: ToolPaneLayout,
    /// When `true`, a second keybind press hides the pane without killing
    /// the underlying process; a third press shows it again with the
    /// scrollback intact. Defaults to `true`.
    pub toggle: bool,
    /// Configured keybinding chord (`<leader>g`, `Ctrl+Shift+t`, …).
    /// Empty when the recipe is palette-only.
    pub keybind: String,
    /// Binary name probed via the `ProcessRunner` seam. Defaults to the
    /// first whitespace-separated token of `command`.
    pub detect: String,
}

impl Default for ToolPaneRecipe {
    fn default() -> Self {
        Self {
            command: String::new(),
            layout: ToolPaneLayout::default(),
            toggle: true,
            keybind: String::new(),
            detect: String::new(),
        }
    }
}

impl ToolPaneRecipe {
    /// The binary name to probe — explicit `detect` if set, otherwise the
    /// first whitespace-separated token of `command`.
    pub fn detect_binary(&self) -> &str {
        if !self.detect.is_empty() {
            return &self.detect;
        }
        self.command
            .split_whitespace()
            .next()
            .unwrap_or(&self.command)
    }
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
    #[error("invalid cockpit layout: {0}")]
    CockpitLayout(String),
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

    #[test]
    fn parses_tool_pane_recipes_with_defaults() {
        let config = Config::from_toml(
            r#"
[panes.tools.lazygit]
command = "lazygit"
layout = "floating"
keybind = "<leader>g"

[panes.tools."claude-code"]
command = "claude --resume"
layout = "side-right"
toggle = false
"#,
        )
        .unwrap();

        let lazygit = config.panes.tools.get("lazygit").expect("lazygit recipe");
        assert_eq!(lazygit.command, "lazygit");
        assert_eq!(lazygit.layout, ToolPaneLayout::Floating);
        assert!(lazygit.toggle, "default toggle is true");
        assert_eq!(lazygit.keybind, "<leader>g");
        assert_eq!(lazygit.detect_binary(), "lazygit");

        let claude = config
            .panes
            .tools
            .get("claude-code")
            .expect("claude-code recipe");
        assert_eq!(claude.command, "claude --resume");
        assert_eq!(claude.layout, ToolPaneLayout::SideRight);
        assert!(!claude.toggle);
        assert_eq!(claude.detect_binary(), "claude");
    }

    #[test]
    fn tool_pane_recipe_uses_explicit_detect_when_set() {
        let config = Config::from_toml(
            r#"
[panes.tools.toy]
command = "cargo run --release --bin custom-tool"
detect  = "cargo"
"#,
        )
        .unwrap();
        let toy = config.panes.tools.get("toy").unwrap();
        assert_eq!(toy.detect_binary(), "cargo");
    }

    #[test]
    fn tool_pane_recipe_unknown_layout_is_an_error() {
        let err = Config::from_toml(
            r#"
[panes.tools.x]
command = "x"
layout  = "windowed"
"#,
        )
        .unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
}
