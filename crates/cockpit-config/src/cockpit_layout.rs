//! Native cockpit multiplexer layout file parsing (KDL) — v0.7 M7.8.
//!
//! The native multiplexer (`cockpit-mux`) replaces the v0.3 Zellij hand-off,
//! so cockpit now owns the on-disk layout schema. The new format mirrors
//! the [`cockpit_mux::LayoutNode`] data model: nested `split` nodes with a
//! direction and ratio, and leaf `pane` nodes with an optional `command`
//! string the multiplexer runs on first attach. No plugin slots, no
//! themes, no swap layouts (AGENTS §2 hard rule #7).
//!
//! Example:
//!
//! ```kdl
//! layout {
//!     split direction="horizontal" ratio=0.6 {
//!         pane command="cargo watch -x test"
//!         split direction="vertical" {
//!             pane
//!             pane command="lazygit"
//!         }
//!     }
//! }
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
};

use kdl::{KdlDocument, KdlNode, KdlValue};

use crate::ConfigError;

/// Logical split direction in a layout description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CockpitSplitDirection {
    /// Left/right split.
    Horizontal,
    /// Top/bottom split.
    Vertical,
}

/// One node in a parsed cockpit layout description. Independent of
/// `cockpit_mux::LayoutNode` so the config layer doesn't depend on the
/// runtime crate.
#[derive(Debug, Clone, PartialEq)]
pub enum CockpitLayoutNode {
    /// Leaf pane with an optional command to run on first attach.
    Pane { command: Option<String> },
    /// Recursive split between two children.
    Split {
        direction: CockpitSplitDirection,
        ratio: f32,
        a: Box<CockpitLayoutNode>,
        b: Box<CockpitLayoutNode>,
    },
}

impl CockpitLayoutNode {
    /// Total leaf-pane count under this node.
    pub fn pane_count(&self) -> usize {
        match self {
            Self::Pane { .. } => 1,
            Self::Split { a, b, .. } => a.pane_count() + b.pane_count(),
        }
    }

    /// Iterate leaf-pane commands in left-to-right / top-to-bottom order.
    pub fn pane_commands(&self) -> Vec<Option<String>> {
        let mut out = Vec::new();
        self.push_pane_commands(&mut out);
        out
    }

    fn push_pane_commands(&self, out: &mut Vec<Option<String>>) {
        match self {
            Self::Pane { command } => out.push(command.clone()),
            Self::Split { a, b, .. } => {
                a.push_pane_commands(out);
                b.push_pane_commands(out);
            }
        }
    }
}

/// A parsed cockpit layout file. `path` is empty for in-memory parses.
#[derive(Debug, Clone, PartialEq)]
pub struct CockpitLayout {
    pub path: PathBuf,
    pub root: CockpitLayoutNode,
}

impl CockpitLayout {
    /// Parse a layout description from a KDL string. The returned layout has
    /// an empty path.
    pub fn from_kdl(input: &str) -> Result<Self, ConfigError> {
        let document =
            KdlDocument::parse(input).map_err(|err| ConfigError::CockpitLayout(err.to_string()))?;
        let layout = document
            .nodes()
            .iter()
            .find(|node| node.name().value() == "layout")
            .ok_or_else(|| {
                ConfigError::CockpitLayout("missing top-level `layout` node".to_string())
            })?;

        let root = build_root(layout)?;
        Ok(Self {
            path: PathBuf::new(),
            root,
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

    /// Total leaf-pane count for the whole layout.
    pub fn pane_count(&self) -> usize {
        self.root.pane_count()
    }
}

/// Build the layout root from the top-level `layout { ... }` node. A layout
/// with no children is rejected (we always start with at least one pane).
fn build_root(layout: &KdlNode) -> Result<CockpitLayoutNode, ConfigError> {
    let Some(children) = layout.children() else {
        return Err(ConfigError::CockpitLayout(
            "`layout` node has no children — at least one `pane` is required".to_string(),
        ));
    };
    let nodes: Vec<&KdlNode> = children.nodes().iter().collect();
    match nodes.as_slice() {
        [] => Err(ConfigError::CockpitLayout(
            "`layout` node has no children — at least one `pane` is required".to_string(),
        )),
        [single] => parse_node(single),
        _ => Err(ConfigError::CockpitLayout(format!(
            "`layout` must contain exactly one root node, found {}",
            nodes.len()
        ))),
    }
}

/// Parse a single `pane` or `split` node into a [`CockpitLayoutNode`].
fn parse_node(node: &KdlNode) -> Result<CockpitLayoutNode, ConfigError> {
    match node.name().value() {
        "pane" => parse_pane(node),
        "split" => parse_split(node),
        other => Err(ConfigError::CockpitLayout(format!(
            "unsupported layout node `{other}` (expected `pane` or `split`)"
        ))),
    }
}

fn parse_pane(node: &KdlNode) -> Result<CockpitLayoutNode, ConfigError> {
    let command = string_property(node, "command")?;
    if node.children().is_some() {
        return Err(ConfigError::CockpitLayout(
            "`pane` nodes must be leaves — splits go on `split` nodes".to_string(),
        ));
    }
    Ok(CockpitLayoutNode::Pane { command })
}

fn parse_split(node: &KdlNode) -> Result<CockpitLayoutNode, ConfigError> {
    let direction = string_property(node, "direction")?
        .ok_or_else(|| {
            ConfigError::CockpitLayout(
                "`split` requires a `direction=\"horizontal\"|\"vertical\"` attribute".to_string(),
            )
        })
        .and_then(|raw| match raw.as_str() {
            "horizontal" => Ok(CockpitSplitDirection::Horizontal),
            "vertical" => Ok(CockpitSplitDirection::Vertical),
            other => Err(ConfigError::CockpitLayout(format!(
                "invalid split direction `{other}` (expected `horizontal` or `vertical`)"
            ))),
        })?;

    let ratio = float_property(node, "ratio")?.unwrap_or(0.5);
    if !(0.0 < ratio && ratio < 1.0) {
        return Err(ConfigError::CockpitLayout(format!(
            "split `ratio` must be in (0, 1), got {ratio}"
        )));
    }

    let Some(children) = node.children() else {
        return Err(ConfigError::CockpitLayout(
            "`split` must contain two child nodes".to_string(),
        ));
    };
    let nodes: Vec<&KdlNode> = children.nodes().iter().collect();
    if nodes.len() != 2 {
        return Err(ConfigError::CockpitLayout(format!(
            "`split` must contain exactly two child nodes, found {}",
            nodes.len()
        )));
    }
    let a = parse_node(nodes[0])?;
    let b = parse_node(nodes[1])?;
    Ok(CockpitLayoutNode::Split {
        direction,
        ratio,
        a: Box::new(a),
        b: Box::new(b),
    })
}

fn string_property(node: &KdlNode, name: &str) -> Result<Option<String>, ConfigError> {
    let Some(entry) = node.entry(name) else {
        return Ok(None);
    };
    match entry.value() {
        KdlValue::String(value) => Ok(Some(value.clone())),
        other => Err(ConfigError::CockpitLayout(format!(
            "expected string for `{name}`, got {other:?}"
        ))),
    }
}

fn float_property(node: &KdlNode, name: &str) -> Result<Option<f32>, ConfigError> {
    let Some(entry) = node.entry(name) else {
        return Ok(None);
    };
    match entry.value() {
        KdlValue::Float(value) => Ok(Some(*value as f32)),
        KdlValue::Integer(value) => Ok(Some(*value as f32)),
        other => Err(ConfigError::CockpitLayout(format!(
            "expected number for `{name}`, got {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LAYOUT: &str = r#"
layout {
    split direction="horizontal" ratio=0.6 {
        pane command="cargo watch -x test"
        split direction="vertical" {
            pane
            pane command="lazygit"
        }
    }
}
"#;

    #[test]
    fn parses_nested_splits_with_commands() {
        let layout = CockpitLayout::from_kdl(SAMPLE_LAYOUT).expect("parse");
        assert_eq!(layout.pane_count(), 3);
        assert_eq!(
            layout.root.pane_commands(),
            vec![
                Some("cargo watch -x test".to_string()),
                None,
                Some("lazygit".to_string()),
            ]
        );
        match &layout.root {
            CockpitLayoutNode::Split {
                direction, ratio, ..
            } => {
                assert_eq!(*direction, CockpitSplitDirection::Horizontal);
                assert!((ratio - 0.6).abs() < f32::EPSILON);
            }
            other => panic!("expected split root, got {other:?}"),
        }
    }

    #[test]
    fn defaults_ratio_to_half_when_omitted() {
        let input = r#"
layout {
    split direction="vertical" {
        pane
        pane
    }
}
"#;
        let layout = CockpitLayout::from_kdl(input).unwrap();
        match &layout.root {
            CockpitLayoutNode::Split { ratio, .. } => {
                assert!((ratio - 0.5).abs() < f32::EPSILON)
            }
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn single_pane_layout_parses() {
        let layout = CockpitLayout::from_kdl("layout { pane }").unwrap();
        assert_eq!(layout.pane_count(), 1);
        match &layout.root {
            CockpitLayoutNode::Pane { command } => assert!(command.is_none()),
            other => panic!("expected pane, got {other:?}"),
        }
    }

    #[test]
    fn missing_layout_node_errors() {
        let err = CockpitLayout::from_kdl("pane").unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("missing top-level")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn empty_layout_errors() {
        let err = CockpitLayout::from_kdl("layout {}").unwrap_err();
        assert!(matches!(err, ConfigError::CockpitLayout(_)));
    }

    #[test]
    fn unknown_node_kind_errors() {
        let err = CockpitLayout::from_kdl("layout { tab { pane } }").unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("unsupported")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn pane_with_children_errors() {
        let err = CockpitLayout::from_kdl("layout { pane { pane } }").unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("must be leaves")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn split_missing_direction_errors() {
        let err = CockpitLayout::from_kdl("layout { split { pane; pane } }").unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("direction")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn split_invalid_direction_errors() {
        let input = r#"
layout {
    split direction="diagonal" {
        pane
        pane
    }
}
"#;
        let err = CockpitLayout::from_kdl(input).unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("invalid split direction")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn split_with_one_child_errors() {
        let input = r#"
layout {
    split direction="vertical" {
        pane
    }
}
"#;
        let err = CockpitLayout::from_kdl(input).unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("exactly two")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn split_ratio_out_of_range_errors() {
        let input = r#"
layout {
    split direction="vertical" ratio=1.5 {
        pane
        pane
    }
}
"#;
        let err = CockpitLayout::from_kdl(input).unwrap_err();
        assert!(
            matches!(&err, ConfigError::CockpitLayout(msg) if msg.contains("ratio")),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn load_reads_file_and_records_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.kdl");
        fs::write(&path, SAMPLE_LAYOUT).unwrap();

        let layout = CockpitLayout::load(&path).unwrap();
        assert_eq!(layout.path, path);
        assert_eq!(layout.pane_count(), 3);
    }

    #[test]
    fn load_propagates_missing_file_as_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = CockpitLayout::load(dir.path().join("nope.kdl")).unwrap_err();
        assert!(matches!(err, ConfigError::Read(_)));
    }
}
