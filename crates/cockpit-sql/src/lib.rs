//! `cockpit-sql` — SQL execution backend (v0.5 M5.1).
//!
//! Spec posture (`IMPLEMENTATION_PLAN.md` §8a):
//!
//! * **No embedded `duckdb` crate.** Cockpit shells out to `mise exec --
//!   duckdb` so the binary stays small and the future instant-load budget
//!   stays intact.
//! * **`mise exec` wrapper.** Every spawn goes through `mise exec` so the
//!   query inherits the project's mise environment (matches LSP M4.5 /
//!   spec §19).
//! * **No auto-install.** If `duckdb` is neither in `[tools]` nor on
//!   `$PATH`, callers surface the standard "detect, surface, prompt"
//!   flow — same shape as the formatter prompt from M4.4 (AGENTS rule #6).
//!
//! Headless: every engine sits behind the [`SqlEngine`] trait, so the
//! notebook view-model (M5.3) and the dbt-lite project mode (M5.6) can
//! unit-test against [`FakeSqlEngine`] without a real DuckDB binary on the
//! test machine. The pattern mirrors the [`TerminalEngine`] trait from
//! `cockpit-terminal` — M4.10 made the env seam that makes this practical.
//!
//! [`TerminalEngine`]: ../cockpit_terminal/engine/trait.TerminalEngine.html

pub mod detect;
pub mod duckdb;
pub mod engine;
pub mod fake;
pub mod ggsql;

pub use detect::{DuckDbAvailability, detect_duckdb, detect_ggsql};
pub use duckdb::DuckDbEngine;
pub use engine::{QueryError, QueryResult, SqlEngine, SqlValue};
pub use fake::FakeSqlEngine;
pub use ggsql::{GgsqlEngine, statement_targets_ggsql};
