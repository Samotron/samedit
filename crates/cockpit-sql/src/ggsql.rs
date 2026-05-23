//! ggsql shell-out engine — v0.5 M5.1a.
//!
//! ggsql (a Posit alpha project) wraps DuckDB and emits Vega-Lite v6 JSON
//! for cells that declare `VISUALISE ...`. We spawn `mise exec -- ggsql
//! exec --reader duckdb://memory --writer vegalite` for each visual cell;
//! the [`SqlEngine`] contract reuses [`QueryResult`] but the rows carry
//! Vega-Lite JSON as a single text column so existing notebook plumbing
//! still works — the chart renderer (M5.5) reaches for the JSON via
//! [`GgsqlEngine::extract_vega_lite`].
//!
//! Detection / prompt copy lives in [`crate::detect`] (sibling module
//! `ggsql_detect`) so the notebook view-model can present the same prompt
//! shape as DuckDB.

use std::path::PathBuf;
use std::sync::Arc;

use cockpit_project::{ProcessRunner, ProcessSpec, StdProcessRunner};

use crate::duckdb::engine_missing;
use crate::engine::{QueryError, QueryResult, SqlEngine, SqlValue};

/// SQL-like engine that pipes statements through `ggsql exec`. Output is
/// Vega-Lite JSON for `VISUALISE` cells; non-visual queries still parse
/// because ggsql forwards them to its embedded DuckDB.
pub struct GgsqlEngine {
    process: Arc<dyn ProcessRunner>,
    root: PathBuf,
}

impl GgsqlEngine {
    /// New engine rooted at `root`, using the std-backed process runner.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            process: Arc::new(StdProcessRunner),
            root: root.into(),
        }
    }

    /// Trait-injected constructor for tests.
    pub fn with_runner(root: impl Into<PathBuf>, process: Arc<dyn ProcessRunner>) -> Self {
        Self {
            process,
            root: root.into(),
        }
    }

    /// Pull the Vega-Lite JSON out of a [`QueryResult`] produced by this
    /// engine. M5.5 (chart renderer) calls this before handing the JSON
    /// to `vl-convert vl2png`.
    pub fn extract_vega_lite(result: &QueryResult) -> Option<&str> {
        let row = result.rows.first()?;
        let value = row.first()?;
        match value {
            SqlValue::String(text) => Some(text.as_str()),
            _ => None,
        }
    }
}

impl SqlEngine for GgsqlEngine {
    fn execute(&self, statement: &str) -> Result<QueryResult, QueryError> {
        let mut sql = statement.trim().to_string();
        if !sql.ends_with(';') {
            sql.push(';');
        }
        let spec = ProcessSpec::new("mise")
            .arg("exec")
            .arg("--")
            .arg("ggsql")
            .arg("exec")
            .arg("--reader")
            .arg("duckdb://memory")
            .arg("--writer")
            .arg("vegalite")
            .arg("-c")
            .arg(&sql)
            .current_dir(&self.root);
        let output = self
            .process
            .run(&spec)
            .map_err(|err| QueryError::Io(err.to_string()))?;
        if !output.success {
            let stderr = output.stderr_string();
            if engine_missing(&stderr) {
                return Err(QueryError::EngineMissing("ggsql".to_string()));
            }
            let excerpt = stderr
                .lines()
                .next()
                .unwrap_or("(no stderr)")
                .chars()
                .take(200)
                .collect();
            return Err(QueryError::EngineError {
                stderr_excerpt: excerpt,
            });
        }
        let stdout = output.stdout_string();
        let trimmed = stdout.trim();
        // ggsql writes a single Vega-Lite JSON document on stdout. We
        // surface it as one row / one column so the notebook view-model
        // can treat it like any other tabular result while the chart
        // renderer reaches for [`extract_vega_lite`].
        if trimmed.is_empty() {
            return Ok(QueryResult::empty());
        }
        Ok(QueryResult {
            columns: vec!["vega_lite".to_string()],
            rows: vec![vec![SqlValue::String(trimmed.to_string())]],
            elapsed_ms: None,
        })
    }

    fn is_available(&self) -> bool {
        let spec = ProcessSpec::new("mise")
            .arg("exec")
            .arg("--")
            .arg("ggsql")
            .arg("--version")
            .current_dir(&self.root);
        self.process
            .run(&spec)
            .map(|output| output.success)
            .unwrap_or(false)
    }
}

/// True when the statement should be routed to ggsql instead of DuckDB.
/// Mirrors the routing rule in the plan (§8a M5.2): cells whose body
/// contains a `VISUALISE` (or `VISUALIZE`) clause are visual cells.
pub fn statement_targets_ggsql(statement: &str) -> bool {
    let upper = statement.to_ascii_uppercase();
    upper.contains("VISUALISE") || upper.contains("VISUALIZE")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::{FakeProcessRunner, ProcessOutput};

    #[test]
    fn statement_routing_detects_both_spellings() {
        assert!(statement_targets_ggsql(
            "SELECT * FROM x VISUALISE DRAW point"
        ));
        assert!(statement_targets_ggsql(
            "select * from x visualize draw bar"
        ));
        assert!(!statement_targets_ggsql("SELECT * FROM x WHERE y > 1"));
    }

    #[test]
    fn execute_returns_vega_lite_payload_as_a_single_row() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "mise",
            [
                "exec".to_string(),
                "--".to_string(),
                "ggsql".to_string(),
                "exec".to_string(),
                "--reader".to_string(),
                "duckdb://memory".to_string(),
                "--writer".to_string(),
                "vegalite".to_string(),
                "-c".to_string(),
                "SELECT x VISUALISE DRAW point;".to_string(),
            ],
            ProcessOutput {
                success: true,
                stdout: br#"{"$schema":"https://vega.github.io/schema/vega-lite/v6.json"}"#
                    .to_vec(),
                stderr: Vec::new(),
            },
        );
        let engine = GgsqlEngine::with_runner("/proj", Arc::new(runner));
        let result = engine.execute("SELECT x VISUALISE DRAW point").unwrap();
        assert_eq!(result.columns, vec!["vega_lite".to_string()]);
        let payload = GgsqlEngine::extract_vega_lite(&result).expect("vega-lite JSON");
        assert!(payload.contains("vega-lite/v6"), "got: {payload}");
    }

    #[test]
    fn execute_distinguishes_missing_binary_from_runtime_error() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "mise",
            [
                "exec".to_string(),
                "--".to_string(),
                "ggsql".to_string(),
                "exec".to_string(),
                "--reader".to_string(),
                "duckdb://memory".to_string(),
                "--writer".to_string(),
                "vegalite".to_string(),
                "-c".to_string(),
                "SELECT 1;".to_string(),
            ],
            ProcessOutput {
                success: false,
                stdout: Vec::new(),
                stderr: b"command not found: ggsql\n".to_vec(),
            },
        );
        let engine = GgsqlEngine::with_runner("/proj", Arc::new(runner));
        let err = engine.execute("SELECT 1").unwrap_err();
        assert!(matches!(err, QueryError::EngineMissing(name) if name == "ggsql"));
    }
}
