//! v0.7 M7.8 binary wire-up: load a project's `cockpit_layout` from
//! metadata and translate it into a [`cockpit_mux::LayoutDescription`]
//! the runtime mux understands.

use std::path::Path;

use cockpit_config::{CockpitLayout, CockpitLayoutNode, CockpitSplitDirection};
use cockpit_mux::{LayoutDescription, SplitDirection};
use cockpit_project::ProjectDetection;

/// Result of resolving and parsing a project's native mux layout.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLayout {
    /// Description ready to feed into [`cockpit_mux::Session::from_layout`].
    pub description: LayoutDescription,
}

/// Look up `metadata.cockpit.cockpit_layout` on `detection`, load the
/// referenced KDL file relative to the project root, and translate it.
/// Returns `Ok(None)` when no layout is configured; `Err(message)` on
/// load or parse failure so the caller can surface it in the status line.
pub fn resolve_cockpit_layout(
    detection: &ProjectDetection,
) -> Result<Option<ResolvedLayout>, String> {
    let Some(configured) = detection
        .mise
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.cockpit_layout.as_deref())
    else {
        return Ok(None);
    };
    let absolute = detection.root_path.join(configured);
    load_layout(&absolute).map(Some)
}

fn load_layout(path: &Path) -> Result<ResolvedLayout, String> {
    let layout = CockpitLayout::load(path).map_err(|err| format!("{err}"))?;
    Ok(ResolvedLayout {
        description: description_from_node(&layout.root),
    })
}

/// Translate a parsed [`CockpitLayoutNode`] into a [`LayoutDescription`].
/// The two trees are isomorphic — the only reason they're separate types is
/// to keep `cockpit-config` from depending on `cockpit-mux`.
pub fn description_from_node(node: &CockpitLayoutNode) -> LayoutDescription {
    match node {
        CockpitLayoutNode::Pane { command } => LayoutDescription::Pane {
            command: command.clone(),
        },
        CockpitLayoutNode::Split {
            direction,
            ratio,
            a,
            b,
        } => LayoutDescription::Split {
            direction: split_direction(*direction),
            ratio: *ratio,
            a: Box::new(description_from_node(a)),
            b: Box::new(description_from_node(b)),
        },
    }
}

fn split_direction(direction: CockpitSplitDirection) -> SplitDirection {
    match direction {
        CockpitSplitDirection::Horizontal => SplitDirection::Horizontal,
        CockpitSplitDirection::Vertical => SplitDirection::Vertical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_mirrors_the_config_node_tree() {
        let node = CockpitLayoutNode::Split {
            direction: CockpitSplitDirection::Horizontal,
            ratio: 0.6,
            a: Box::new(CockpitLayoutNode::Pane {
                command: Some("editor".to_string()),
            }),
            b: Box::new(CockpitLayoutNode::Split {
                direction: CockpitSplitDirection::Vertical,
                ratio: 0.4,
                a: Box::new(CockpitLayoutNode::Pane { command: None }),
                b: Box::new(CockpitLayoutNode::Pane {
                    command: Some("lazygit".to_string()),
                }),
            }),
        };
        match description_from_node(&node) {
            LayoutDescription::Split {
                direction,
                ratio,
                a,
                b,
            } => {
                assert_eq!(direction, SplitDirection::Horizontal);
                assert!((ratio - 0.6).abs() < f32::EPSILON);
                assert_eq!(
                    *a,
                    LayoutDescription::Pane {
                        command: Some("editor".to_string()),
                    }
                );
                match *b {
                    LayoutDescription::Split { ratio, .. } => {
                        assert!((ratio - 0.4).abs() < f32::EPSILON)
                    }
                    other => panic!("expected nested split, got {other:?}"),
                }
            }
            other => panic!("expected split root, got {other:?}"),
        }
    }
}
