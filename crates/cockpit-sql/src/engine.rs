//! Engine trait and result types shared by every SQL backend.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One value in a [`QueryResult`] row. Mirrors the smallest useful subset of
/// DuckDB's JSON output — everything that does not parse into a typed value
/// degrades to [`SqlValue::String`] so callers never lose data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SqlValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

impl SqlValue {
    /// Plain text rendering for terminal/grid display. `Null` prints as the
    /// literal `NULL` so it is visually distinct from an empty string.
    pub fn display(&self) -> String {
        match self {
            SqlValue::Null => "NULL".to_string(),
            SqlValue::Bool(value) => value.to_string(),
            SqlValue::Integer(value) => value.to_string(),
            SqlValue::Float(value) => value.to_string(),
            SqlValue::String(value) => value.clone(),
        }
    }
}

impl From<serde_json::Value> for SqlValue {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => SqlValue::Null,
            serde_json::Value::Bool(value) => SqlValue::Bool(value),
            serde_json::Value::Number(value) => {
                if let Some(int) = value.as_i64() {
                    SqlValue::Integer(int)
                } else if let Some(float) = value.as_f64() {
                    SqlValue::Float(float)
                } else {
                    SqlValue::String(value.to_string())
                }
            }
            serde_json::Value::String(value) => SqlValue::String(value),
            other => SqlValue::String(other.to_string()),
        }
    }
}

/// One executed query's tabular output. DuckDB returns either rows or an
/// error; statements that emit no rows (e.g. `CREATE TABLE`) come back with
/// empty `columns` and `rows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    /// Column names in declaration order.
    pub columns: Vec<String>,
    /// Rows; each row has one entry per column.
    pub rows: Vec<Vec<SqlValue>>,
    /// Total elapsed time the engine reported (informational only — not
    /// every backend provides it).
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
}

impl QueryResult {
    /// Empty result — used for DDL statements that produce no rows.
    pub fn empty() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            elapsed_ms: None,
        }
    }

    /// True when the engine returned no rows. Useful for the notebook UI
    /// to decide whether to render the grid or a "0 rows" placeholder.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Typed errors a SQL engine can raise. Backends translate their native
/// errors into one of these so callers stay backend-agnostic.
#[derive(Debug, Error)]
pub enum QueryError {
    /// The engine binary is not available (not in `[tools]`, not on
    /// `$PATH`). Carries the suggested binary name so the caller can
    /// surface a prompt.
    #[error("SQL engine `{0}` is not installed")]
    EngineMissing(String),
    /// The engine ran but returned an error. `stderr_excerpt` is a short,
    /// status-line-friendly snippet; the full text lives in tracing logs.
    #[error("SQL engine error: {stderr_excerpt}")]
    EngineError {
        /// First line of the engine's stderr, capped at 200 characters.
        stderr_excerpt: String,
    },
    /// The engine emitted output that did not parse as the documented
    /// format (typically JSON). Carries the raw text for diagnostics.
    #[error("SQL engine returned unparseable output: {reason}")]
    ParseError { reason: String, raw: String },
    /// Process spawn or I/O failure — surfaced verbatim.
    #[error("SQL engine I/O failed: {0}")]
    Io(String),
}

/// One SQL engine. `execute` is the only method backends must implement;
/// `is_available` defaults to "always" so fakes and embedded engines do not
/// need to lie about a non-existent binary.
pub trait SqlEngine: Send + Sync {
    /// Run `statement` and return the resulting rows. DDL and DML come back
    /// with an empty [`QueryResult`]. SELECT-like statements come back with
    /// columns + rows. The engine implementation decides whether the
    /// invocation is one-shot or replays into a persistent session.
    fn execute(&self, statement: &str) -> Result<QueryResult, QueryError>;

    /// True when the underlying engine is available to spawn — equivalent
    /// to "we expect [`execute`](Self::execute) to do useful work". Default
    /// implementation returns true: fakes and embedded backends never have
    /// an availability problem.
    fn is_available(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_value_display_distinguishes_null_from_empty_string() {
        assert_eq!(SqlValue::Null.display(), "NULL");
        assert_eq!(SqlValue::String(String::new()).display(), "");
        assert_eq!(SqlValue::Integer(42).display(), "42");
        assert_eq!(SqlValue::Bool(true).display(), "true");
    }

    #[test]
    fn sql_value_from_json_promotes_typed_atoms() {
        assert_eq!(SqlValue::from(serde_json::Value::Null), SqlValue::Null);
        assert_eq!(
            SqlValue::from(serde_json::Value::Bool(false)),
            SqlValue::Bool(false)
        );
        assert_eq!(SqlValue::from(serde_json::json!(7)), SqlValue::Integer(7));
        assert_eq!(SqlValue::from(serde_json::json!(3.5)), SqlValue::Float(3.5));
        assert_eq!(
            SqlValue::from(serde_json::json!("hello")),
            SqlValue::String("hello".to_string())
        );
    }

    #[test]
    fn query_result_helpers_round_trip_empty_results() {
        let empty = QueryResult::empty();
        assert!(empty.is_empty());
        assert!(empty.columns.is_empty());
        assert!(empty.rows.is_empty());
    }
}
