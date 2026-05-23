//! Detect whether DuckDB is reachable for this project.
//!
//! Mirrors the formatter detection from M4.4 — pure decision logic over
//! `MiseProject` + a `BinaryLookup`, with the caller responsible for the
//! prompt UI. AGENTS rule #6 means cockpit never auto-installs duckdb; we
//! only report whether it is reachable and, when not, suggest the
//! `mise use cargo:duckdb-cli` invocation users should run by hand.

use cockpit_project::{BinaryLookup, MiseProject};

/// Where DuckDB lives — drives the prompt copy the notebook view-model
/// surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuckDbAvailability {
    /// Declared in `mise.toml [tools]` (preferred — the project pins a
    /// reproducible version).
    InMiseTools,
    /// On `$PATH` but not in `[tools]`. Still works; the notebook UI
    /// nudges the user to pin a version.
    OnPath,
    /// Neither in `[tools]` nor on `$PATH`. The notebook UI surfaces the
    /// "add to mise.toml?" prompt.
    Missing,
}

impl DuckDbAvailability {
    /// True when DuckDB is reachable today (the engine will be able to
    /// spawn).
    pub fn reachable(&self) -> bool {
        !matches!(self, Self::Missing)
    }
}

/// Decide whether DuckDB is available for `project`. Production callers
/// pass `cockpit_project::PathBinaryLookup`; tests pass
/// `cockpit_project::FixedBinaryLookup` so the answer is deterministic.
pub fn detect_duckdb(project: &MiseProject, lookup: &dyn BinaryLookup) -> DuckDbAvailability {
    detect_named(project, lookup, "duckdb")
}

/// Same shape as [`detect_duckdb`] but for `ggsql` (M5.1a). Kept as its
/// own entry point so callers can present a distinct prompt and the
/// notebook code does not need to repeat the binary name.
pub fn detect_ggsql(project: &MiseProject, lookup: &dyn BinaryLookup) -> DuckDbAvailability {
    detect_named(project, lookup, "ggsql")
}

fn detect_named(
    project: &MiseProject,
    lookup: &dyn BinaryLookup,
    binary: &str,
) -> DuckDbAvailability {
    if project.tools.iter().any(|tool| tool.name == binary) {
        return DuckDbAvailability::InMiseTools;
    }
    if lookup.exists(binary) {
        return DuckDbAvailability::OnPath;
    }
    DuckDbAvailability::Missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::{FixedBinaryLookup, NoBinaryLookup, Tool};

    fn project_with_tools(tools: Vec<&str>) -> MiseProject {
        MiseProject {
            tools: tools
                .into_iter()
                .map(|name| Tool {
                    name: name.to_string(),
                    version: "latest".to_string(),
                })
                .collect(),
            ..MiseProject::default()
        }
    }

    #[test]
    fn mise_tools_entry_wins_over_path() {
        let project = project_with_tools(vec!["duckdb"]);
        let lookup = FixedBinaryLookup::new(["duckdb"]);
        assert_eq!(
            detect_duckdb(&project, &lookup),
            DuckDbAvailability::InMiseTools
        );
        assert!(DuckDbAvailability::InMiseTools.reachable());
    }

    #[test]
    fn path_only_falls_back_to_on_path() {
        let project = MiseProject::default();
        let lookup = FixedBinaryLookup::new(["duckdb"]);
        assert_eq!(detect_duckdb(&project, &lookup), DuckDbAvailability::OnPath);
        assert!(DuckDbAvailability::OnPath.reachable());
    }

    #[test]
    fn neither_tools_nor_path_reports_missing() {
        let project = MiseProject::default();
        assert_eq!(
            detect_duckdb(&project, &NoBinaryLookup),
            DuckDbAvailability::Missing
        );
        assert!(!DuckDbAvailability::Missing.reachable());
    }
}
