//! In-memory `SqlEngine` for hermetic tests.
//!
//! Notebook (M5.3+) and dbt-lite (M5.6+) tests script the engine's
//! responses up front, then assert that the right statements were run.
//! Mirrors the `FakeProcessRunner` pattern from `cockpit-project::env`
//! (M4.10) — request → scripted response, recorded in a call log.

use std::sync::Mutex;

use crate::engine::{QueryError, QueryResult, SqlEngine};

#[derive(Debug, Default)]
pub struct FakeSqlEngine {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Scripted responses popped in FIFO order. An unscripted call
    /// returns [`QueryError::EngineError`] so tests have to opt into
    /// every statement they expect.
    responses: Vec<Result<QueryResult, QueryError>>,
    /// Statements that landed on the engine, in call order.
    log: Vec<String>,
    available: bool,
}

impl FakeSqlEngine {
    /// New engine with no scripted responses (so any call fails) and
    /// `is_available == true`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                responses: Vec::new(),
                log: Vec::new(),
                available: true,
            }),
        }
    }

    /// Convenience: a fake that reports the engine as not installed, so
    /// callers can exercise the "duckdb missing" branch (M5.1 prompt).
    pub fn unavailable() -> Self {
        Self {
            inner: Mutex::new(Inner {
                responses: Vec::new(),
                log: Vec::new(),
                available: false,
            }),
        }
    }

    /// Push one scripted success — the next [`SqlEngine::execute`] call
    /// returns `result`.
    pub fn expect(&self, result: QueryResult) {
        self.inner
            .lock()
            .expect("FakeSqlEngine poisoned")
            .responses
            .push(Ok(result));
    }

    /// Push one scripted failure.
    pub fn expect_err(&self, err: QueryError) {
        self.inner
            .lock()
            .expect("FakeSqlEngine poisoned")
            .responses
            .push(Err(err));
    }

    /// Snapshot of every statement the engine has been asked to run.
    pub fn calls(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("FakeSqlEngine poisoned")
            .log
            .clone()
    }
}

impl SqlEngine for FakeSqlEngine {
    fn execute(&self, statement: &str) -> Result<QueryResult, QueryError> {
        let mut inner = self.inner.lock().expect("FakeSqlEngine poisoned");
        inner.log.push(statement.to_string());
        if inner.responses.is_empty() {
            return Err(QueryError::EngineError {
                stderr_excerpt: format!("FakeSqlEngine: no scripted response for {statement:?}"),
            });
        }
        inner.responses.remove(0)
    }

    fn is_available(&self) -> bool {
        self.inner.lock().expect("FakeSqlEngine poisoned").available
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{QueryResult, SqlValue};

    #[test]
    fn scripted_responses_pop_in_fifo_order() {
        let engine = FakeSqlEngine::new();
        engine.expect(QueryResult {
            columns: vec!["a".to_string()],
            rows: vec![vec![SqlValue::Integer(1)]],
            elapsed_ms: None,
        });
        engine.expect(QueryResult::empty());

        let first = engine.execute("SELECT 1").unwrap();
        assert_eq!(first.rows[0][0], SqlValue::Integer(1));

        let second = engine.execute("CREATE TABLE t (n INT)").unwrap();
        assert!(second.is_empty());
    }

    #[test]
    fn unscripted_calls_fail_so_tests_have_to_opt_in() {
        let engine = FakeSqlEngine::new();
        let err = engine.execute("SELECT 1").unwrap_err();
        assert!(matches!(err, QueryError::EngineError { .. }));
    }

    #[test]
    fn unavailable_fake_reports_is_available_false() {
        let engine = FakeSqlEngine::unavailable();
        assert!(!engine.is_available());
    }

    #[test]
    fn calls_log_records_every_executed_statement() {
        let engine = FakeSqlEngine::new();
        engine.expect(QueryResult::empty());
        engine.expect_err(QueryError::EngineError {
            stderr_excerpt: "boom".to_string(),
        });
        let _ = engine.execute("SELECT 1");
        let _ = engine.execute("SELECT 2");
        assert_eq!(
            engine.calls(),
            vec!["SELECT 1".to_string(), "SELECT 2".to_string()]
        );
    }
}
