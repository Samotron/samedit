//! Read-time dependency DAG — M5.9.
//!
//! Built by scanning each model's source for `{{ ref('...') }}` calls.
//! No background indexer — the DAG is recomputed on save (spec §3.9 /
//! §24). Topological ordering uses Kahn's algorithm with stable input
//! order, so the DAG view is deterministic across rebuilds.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::detect::Model;

/// One node in the model DAG. Carries everything the DAG view needs to
/// render the model card without re-reading the source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelNode {
    /// Model name.
    pub name: String,
    /// Names of models this model directly depends on (via
    /// `{{ ref('...') }}`). Sorted alphabetically for stable output.
    pub dependencies: Vec<String>,
    /// Names of models that depend on this one. Maintained alongside
    /// `dependencies` so the UI can render upstream/downstream views
    /// without re-traversing the graph.
    pub dependents: Vec<String>,
}

/// Full DAG. Nodes are keyed by model name; topological order is
/// available via [`Self::topological_order`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDag {
    nodes: BTreeMap<String, ModelNode>,
}

impl ModelDag {
    /// Build a DAG from `models`. Unknown `ref(...)` targets are
    /// dropped silently — the templating layer raises the user-facing
    /// error when the model actually runs; the DAG view stays
    /// permissive so half-typed models still render.
    pub fn from_models(models: &[Model]) -> Self {
        let known: BTreeSet<&str> = models.iter().map(|m| m.name.as_str()).collect();
        let mut nodes: BTreeMap<String, ModelNode> = BTreeMap::new();
        for model in models {
            let mut deps: BTreeSet<String> = BTreeSet::new();
            for name in extract_refs(&model.source) {
                if known.contains(name.as_str()) && name != model.name {
                    deps.insert(name);
                }
            }
            nodes.insert(
                model.name.clone(),
                ModelNode {
                    name: model.name.clone(),
                    dependencies: deps.into_iter().collect(),
                    dependents: Vec::new(),
                },
            );
        }
        // Second pass: fill in `dependents` so the UI can render both
        // directions without re-walking the graph.
        let names: Vec<String> = nodes.keys().cloned().collect();
        for name in &names {
            let deps = nodes[name].dependencies.clone();
            for dep in deps {
                if let Some(parent) = nodes.get_mut(&dep) {
                    parent.dependents.push(name.clone());
                }
            }
        }
        for node in nodes.values_mut() {
            node.dependents.sort();
        }
        Self { nodes }
    }

    /// Borrow the node for `name`, if it exists.
    pub fn node(&self, name: &str) -> Option<&ModelNode> {
        self.nodes.get(name)
    }

    /// Every node in alphabetical order (matches `BTreeMap` iteration).
    pub fn nodes(&self) -> impl Iterator<Item = &ModelNode> {
        self.nodes.values()
    }

    /// Topological order: every node appears after all of its
    /// dependencies. Returns `Err(Cycle)` when the DAG is not a DAG —
    /// the [`crate::materialise::build_plan`] caller surfaces this so
    /// the UI can highlight the offending models.
    pub fn topological_order(&self) -> Result<Vec<String>, DagError> {
        let mut indegree: BTreeMap<&str, usize> =
            self.nodes.keys().map(|name| (name.as_str(), 0)).collect();
        for node in self.nodes.values() {
            for dep in &node.dependents {
                // `dep` depends on the current node; bump its indegree.
                if let Some(entry) = indegree.get_mut(dep.as_str()) {
                    *entry += 1;
                }
            }
        }
        let mut ready: Vec<&str> = indegree
            .iter()
            .filter_map(|(name, count)| if *count == 0 { Some(*name) } else { None })
            .collect();
        ready.sort();
        let mut out: Vec<String> = Vec::with_capacity(self.nodes.len());
        let mut seen: HashSet<&str> = HashSet::new();
        while let Some(next) = ready.pop() {
            if !seen.insert(next) {
                continue;
            }
            out.push(next.to_string());
            let dependents = self
                .nodes
                .get(next)
                .map(|node| node.dependents.clone())
                .unwrap_or_default();
            for dep in dependents {
                if let Some(count) = indegree.get_mut(dep.as_str()) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.push(self.nodes.get_key_value(dep.as_str()).unwrap().0.as_str());
                        ready.sort();
                    }
                }
            }
        }
        if out.len() != self.nodes.len() {
            // Anything we couldn't schedule is part of a cycle (or
            // depends on one). Return the offending names so the UI
            // can highlight them.
            let unscheduled: Vec<String> = self
                .nodes
                .keys()
                .filter(|name| !out.contains(name))
                .cloned()
                .collect();
            return Err(DagError::Cycle(unscheduled));
        }
        Ok(out)
    }
}

/// DAG errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum DagError {
    /// One or more models form a cycle. Carries the names of the
    /// models we could not schedule.
    #[error("model dependency cycle: {0:?}")]
    Cycle(Vec<String>),
}

/// Walk `source` looking for `{{ ref('name') }}` calls. Returns the
/// extracted names verbatim. Whitespace inside the call is tolerated
/// (matches what `cockpit_analytics::template` accepts).
fn extract_refs(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor + 1 < bytes.len() {
        if &bytes[cursor..cursor + 2] != b"{{" {
            cursor += 1;
            continue;
        }
        let start = cursor + 2;
        let Some(end_rel) = source[start..].find("}}") else {
            break;
        };
        let expr = source[start..start + end_rel].trim();
        cursor = start + end_rel + 2;
        if let Some(rest) = expr.strip_prefix("ref(").and_then(|s| s.strip_suffix(')')) {
            let name = rest.trim().trim_matches(|c| c == '"' || c == '\'');
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Materialisation;
    use std::path::PathBuf;

    fn model(name: &str, source: &str) -> Model {
        Model {
            name: name.to_string(),
            path: PathBuf::from(format!("models/{name}.sql")),
            source: source.to_string(),
            materialisation: Materialisation::View,
        }
    }

    #[test]
    fn dag_records_direct_dependencies() {
        let models = vec![
            model("raw_orders", "SELECT 1"),
            model("stg_orders", "SELECT * FROM {{ ref('raw_orders') }}"),
            model(
                "fct_orders",
                "SELECT * FROM {{ ref('stg_orders') }} JOIN {{ ref('raw_orders') }}",
            ),
        ];
        let dag = ModelDag::from_models(&models);
        assert_eq!(
            dag.node("stg_orders").unwrap().dependencies,
            vec!["raw_orders".to_string()]
        );
        let fct = dag.node("fct_orders").unwrap();
        assert_eq!(fct.dependencies, vec!["raw_orders", "stg_orders"]);
        assert_eq!(
            dag.node("raw_orders").unwrap().dependents,
            vec!["fct_orders", "stg_orders"]
        );
    }

    #[test]
    fn topological_order_lists_dependencies_first() {
        let models = vec![
            model("fct_orders", "SELECT * FROM {{ ref('stg_orders') }}"),
            model("stg_orders", "SELECT * FROM {{ ref('raw_orders') }}"),
            model("raw_orders", "SELECT 1"),
        ];
        let dag = ModelDag::from_models(&models);
        let order = dag.topological_order().unwrap();
        assert_eq!(order, vec!["raw_orders", "stg_orders", "fct_orders"]);
    }

    #[test]
    fn unknown_refs_are_dropped_silently() {
        let models = vec![model(
            "stg_orders",
            "SELECT * FROM {{ ref('does_not_exist') }}",
        )];
        let dag = ModelDag::from_models(&models);
        let node = dag.node("stg_orders").unwrap();
        assert!(node.dependencies.is_empty());
    }

    #[test]
    fn cycles_surface_via_dagerror_cycle() {
        let models = vec![
            model("a", "SELECT * FROM {{ ref('b') }}"),
            model("b", "SELECT * FROM {{ ref('a') }}"),
        ];
        let dag = ModelDag::from_models(&models);
        let err = dag.topological_order().unwrap_err();
        match err {
            DagError::Cycle(names) => {
                assert!(names.contains(&"a".to_string()));
                assert!(names.contains(&"b".to_string()));
            }
        }
    }

    #[test]
    fn self_references_are_ignored() {
        let models = vec![model("a", "SELECT * FROM {{ ref('a') }}")];
        let dag = ModelDag::from_models(&models);
        let order = dag.topological_order().unwrap();
        assert_eq!(order, vec!["a".to_string()]);
    }
}
