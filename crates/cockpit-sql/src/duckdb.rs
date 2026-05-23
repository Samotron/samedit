//! DuckDB shell-out engine — the spec'd v0.5 M5.1 backend.
//!
//! Every query becomes one `mise exec -- duckdb -json` spawn for now: a
//! one-shot subprocess that reads the statement on stdin and writes
//! JSON-encoded rows on stdout. The plan calls for a long-running session
//! per project (spec §8a M5.1) as an optimisation; that lands once the
//! notebook view-model is wired up and the latency cost is observable. The
//! [`SqlEngine`] trait stays the same either way — the change is internal.
//!
//! Spawning goes through `cockpit_project::ProcessRunner` (M4.10) so the
//! notebook layer can run hermetic tests without a real `duckdb` on the
//! test box.

use std::path::PathBuf;
use std::sync::Arc;

use cockpit_project::{ProcessRunner, ProcessSpec, StdProcessRunner};

use crate::engine::{QueryError, QueryResult, SqlEngine, SqlValue};

/// SQL engine that talks to DuckDB by spawning `mise exec -- duckdb`.
pub struct DuckDbEngine {
    /// Process-spawning seam (M4.10). Production wires `StdProcessRunner`;
    /// the notebook tests pass a `FakeProcessRunner` so no real duckdb
    /// binary is required.
    process: Arc<dyn ProcessRunner>,
    /// Project root the engine should run in. Each spawn uses this as
    /// `current_dir` so relative `read_csv(...)` paths Just Work.
    root: PathBuf,
}

impl DuckDbEngine {
    /// New engine rooted at `root`, using the std-backed process runner.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            process: Arc::new(StdProcessRunner),
            root: root.into(),
        }
    }

    /// New engine that spawns through the supplied [`ProcessRunner`] — the
    /// seam every notebook / dbt-lite test relies on.
    pub fn with_runner(root: impl Into<PathBuf>, process: Arc<dyn ProcessRunner>) -> Self {
        Self {
            process,
            root: root.into(),
        }
    }
}

impl SqlEngine for DuckDbEngine {
    fn execute(&self, statement: &str) -> Result<QueryResult, QueryError> {
        // -json emits a JSON array of objects (column → value), one per
        // result row. Empty results (DDL) come back as `[]`.
        //
        // We pass the SQL via -c so we never need an interactive prompt.
        // The trailing `;` makes DuckDB happy when the caller forgot one.
        let mut sql = statement.trim().to_string();
        if !sql.ends_with(';') {
            sql.push(';');
        }
        let spec = ProcessSpec::new("mise")
            .arg("exec")
            .arg("--")
            .arg("duckdb")
            .arg("-json")
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
                return Err(QueryError::EngineMissing("duckdb".to_string()));
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
        parse_duckdb_json(&output.stdout_string())
    }

    fn is_available(&self) -> bool {
        // Cheap availability probe: `mise exec -- duckdb --version` exits
        // 0 when duckdb is reachable through mise (which itself falls
        // through to PATH when the project does not pin a version).
        let spec = ProcessSpec::new("mise")
            .arg("exec")
            .arg("--")
            .arg("duckdb")
            .arg("--version")
            .current_dir(&self.root);
        self.process
            .run(&spec)
            .map(|output| output.success)
            .unwrap_or(false)
    }
}

/// True when `stderr` matches one of the strings DuckDB / mise emit when
/// the binary cannot be located. Used so callers can surface the
/// "install duckdb?" prompt instead of a generic "engine error". Shared
/// with the ggsql backend ([`crate::ggsql`]) so both engines classify
/// failures the same way.
pub(crate) fn engine_missing(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("no such file or directory")
        || lower.contains("not found")
        || lower.contains("unknown command")
        || lower.contains("command not found")
}

/// Parse DuckDB's `-json` output: an array of objects, each keyed by column
/// name. Order of columns inside each object is preserved by `serde_json`
/// when `preserve_order` is enabled; we cope without that feature by
/// reading the first object's keys as the column order.
pub(crate) fn parse_duckdb_json(stdout: &str) -> Result<QueryResult, QueryError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(QueryResult::empty());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|err| QueryError::ParseError {
            reason: err.to_string(),
            raw: trimmed.to_string(),
        })?;
    let array = parsed.as_array().ok_or_else(|| QueryError::ParseError {
        reason: "expected a JSON array".to_string(),
        raw: trimmed.to_string(),
    })?;
    if array.is_empty() {
        return Ok(QueryResult::empty());
    }
    let first = array
        .first()
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| QueryError::ParseError {
            reason: "expected an array of objects".to_string(),
            raw: trimmed.to_string(),
        })?;
    let columns: Vec<String> = first.keys().cloned().collect();
    let mut rows = Vec::with_capacity(array.len());
    for value in array {
        let obj = value.as_object().ok_or_else(|| QueryError::ParseError {
            reason: "row is not a JSON object".to_string(),
            raw: trimmed.to_string(),
        })?;
        let row = columns
            .iter()
            .map(|column| {
                obj.get(column)
                    .cloned()
                    .map(SqlValue::from)
                    .unwrap_or(SqlValue::Null)
            })
            .collect();
        rows.push(row);
    }
    Ok(QueryResult {
        columns,
        rows,
        elapsed_ms: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::{FakeProcessRunner, ProcessOutput};

    fn runner(output: ProcessOutput) -> Arc<dyn ProcessRunner> {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "mise",
            [
                "exec".to_string(),
                "--".to_string(),
                "duckdb".to_string(),
                "-json".to_string(),
                "-c".to_string(),
                "SELECT 1 AS n;".to_string(),
            ],
            output,
        );
        Arc::new(runner)
    }

    #[test]
    fn parse_duckdb_json_handles_an_empty_array_as_an_empty_result() {
        let result = parse_duckdb_json("[]").unwrap();
        assert!(result.is_empty());
        assert!(result.columns.is_empty());
    }

    #[test]
    fn parse_duckdb_json_extracts_columns_from_the_first_row() {
        let raw = r#"[{"a":1,"b":"x"},{"a":2,"b":"y"}]"#;
        let result = parse_duckdb_json(raw).unwrap();
        assert_eq!(result.columns, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], SqlValue::Integer(1));
        assert_eq!(result.rows[0][1], SqlValue::String("x".to_string()));
        assert_eq!(result.rows[1][0], SqlValue::Integer(2));
        assert_eq!(result.rows[1][1], SqlValue::String("y".to_string()));
    }

    #[test]
    fn parse_duckdb_json_treats_missing_keys_as_null() {
        let raw = r#"[{"a":1,"b":"x"},{"a":2}]"#;
        let result = parse_duckdb_json(raw).unwrap();
        assert_eq!(result.rows[1][1], SqlValue::Null);
    }

    #[test]
    fn parse_duckdb_json_rejects_non_array_payloads() {
        let err = parse_duckdb_json("{\"a\":1}").unwrap_err();
        assert!(matches!(err, QueryError::ParseError { .. }));
    }

    #[test]
    fn execute_routes_through_the_process_runner_and_returns_rows() {
        let runner = runner(ProcessOutput {
            success: true,
            stdout: b"[{\"n\":1}]".to_vec(),
            stderr: Vec::new(),
        });
        let engine = DuckDbEngine::with_runner("/proj", runner);
        let result = engine.execute("SELECT 1 AS n").unwrap();
        assert_eq!(result.columns, vec!["n".to_string()]);
        assert_eq!(result.rows[0][0], SqlValue::Integer(1));
    }

    #[test]
    fn execute_surfaces_engine_missing_separately_from_runtime_errors() {
        let runner = runner(ProcessOutput {
            success: false,
            stdout: Vec::new(),
            stderr: b"mise: command not found: duckdb\n".to_vec(),
        });
        let engine = DuckDbEngine::with_runner("/proj", runner);
        let err = engine.execute("SELECT 1 AS n").unwrap_err();
        assert!(matches!(err, QueryError::EngineMissing(name) if name == "duckdb"));
    }

    #[test]
    fn execute_surfaces_runtime_errors_with_the_first_stderr_line() {
        let runner = runner(ProcessOutput {
            success: false,
            stdout: Vec::new(),
            stderr: b"Parser Error: syntax error at end of input\n".to_vec(),
        });
        let engine = DuckDbEngine::with_runner("/proj", runner);
        let err = engine.execute("SELECT 1 AS n").unwrap_err();
        match err {
            QueryError::EngineError { stderr_excerpt } => {
                assert!(stderr_excerpt.starts_with("Parser Error"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
