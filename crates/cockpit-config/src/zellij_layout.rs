//! Zellij layout file parsing (KDL).
//!
//! Cockpit hands the configured layout file to Zellij verbatim; this module
//! parses it first so a broken layout is reported by cockpit before Zellij
//! sees it, and so the UI can summarise the layout (tab/pane counts).

use std::{
    fs,
    path::{Path, PathBuf},
};

use kdl::{KdlDocument, KdlNode};

use crate::ConfigError;

/// A parsed Zellij layout file (spec §9 `[metadata.cockpit].zellij_layout`,
/// spec §10 v0.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijLayout {
    /// Absolute path the layout was loaded from. Empty for in-memory parses.
    pub path: PathBuf,
    /// Number of top-level `tab` nodes inside `layout`.
    pub tabs: usize,
    /// Number of top-level `pane` nodes inside `layout`.
    pub panes: usize,
}

impl ZellijLayout {
    /// Parse a layout from a KDL string. The returned layout has an empty path.
    pub fn from_kdl(input: &str) -> Result<Self, ConfigError> {
        let document =
            KdlDocument::parse(input).map_err(|err| ConfigError::ZellijLayout(err.to_string()))?;
        let layout = document
            .nodes()
            .iter()
            .find(|node| node.name().value() == "layout")
            .ok_or_else(|| {
                ConfigError::ZellijLayout("missing top-level `layout` node".to_string())
            })?;

        let (tabs, panes) = count_tabs_and_panes(layout);
        Ok(Self {
            path: PathBuf::new(),
            tabs,
            panes,
        })
    }

    /// Load and parse a layout file from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let input = fs::read_to_string(path).map_err(ConfigError::Read)?;
        let mut layout = Self::from_kdl(&input)?;
        layout.path = path.to_path_buf();
        Ok(layout)
    }
}

fn count_tabs_and_panes(layout: &KdlNode) -> (usize, usize) {
    let Some(children) = layout.children() else {
        return (0, 0);
    };
    let mut tabs = 0;
    let mut panes = 0;
    for child in children.nodes() {
        match child.name().value() {
            "tab" => tabs += 1,
            "pane" => panes += 1,
            _ => {}
        }
    }
    (tabs, panes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LAYOUT: &str = r#"
layout {
    pane size=1 borderless=true {
        plugin location="zellij:tab-bar"
    }
    pane split_direction="vertical" {
        pane
        pane
    }
    pane size=2 borderless=true {
        plugin location="zellij:status-bar"
    }
}
"#;

    #[test]
    fn parses_layout_and_counts_top_level_panes() {
        let layout = ZellijLayout::from_kdl(SAMPLE_LAYOUT).unwrap();
        assert_eq!(layout.panes, 3);
        assert_eq!(layout.tabs, 0);
        assert!(layout.path.as_os_str().is_empty());
    }

    #[test]
    fn parses_tabs() {
        let input = r#"
layout {
    tab name="code" {
        pane
    }
    tab name="tests" {
        pane
        pane
    }
}
"#;
        let layout = ZellijLayout::from_kdl(input).unwrap();
        assert_eq!(layout.tabs, 2);
        assert_eq!(layout.panes, 0);
    }

    #[test]
    fn missing_layout_node_is_error() {
        let err = ZellijLayout::from_kdl("pane\n").unwrap_err();
        assert!(
            matches!(&err, ConfigError::ZellijLayout(msg) if msg.contains("missing top-level")),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn malformed_kdl_is_error() {
        let err = ZellijLayout::from_kdl("layout { pane").unwrap_err();
        assert!(matches!(err, ConfigError::ZellijLayout(_)));
    }

    #[test]
    fn load_reads_file_and_records_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dev.kdl");
        fs::write(&path, SAMPLE_LAYOUT).unwrap();

        let layout = ZellijLayout::load(&path).unwrap();
        assert_eq!(layout.path, path);
        assert_eq!(layout.panes, 3);
    }

    #[test]
    fn load_propagates_missing_file_as_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = ZellijLayout::load(dir.path().join("nope.kdl")).unwrap_err();
        assert!(matches!(err, ConfigError::Read(_)));
    }
}
