//! Cold-start budget check — v0.6 M6.1.
//!
//! Opt-in via `--features bench` so the default `cargo test` stays
//! hermetic. The CI bench leg runs this and fails the build when the
//! per-phase budget regresses; budgets are deliberately generous on a
//! laptop-grade box (the v0.6 targets in `IMPLEMENTATION_PLAN.md`
//! §8b are for the hot path on real hardware — CI doesn't measure
//! that, it just guards against accidental quadratic walks).

#![cfg(feature = "bench")]

use std::time::Duration;

use cockpit_project::{FileTree, detect_project};
use cockpit_testkit::{fixture_path, format_measurements, measure_phase, total};

/// CI-relevant budget. Sized so the test still passes on a slow
/// shared runner; tighter budgets land in the M6.2 / M6.4 work where
/// we're actually optimising frame 1.
const COLD_START_BUDGET: Duration = Duration::from_millis(500);

#[test]
fn cold_start_stays_under_the_budget_on_the_rust_basic_fixture() {
    let path = fixture_path("rust-basic");
    let (detection, detect) = measure_phase("startup.detect", || {
        detect_project(&path).expect("detect rust-basic fixture")
    });
    let (_tree, tree) = measure_phase("startup.tree", || FileTree::load(&path).expect("tree"));
    let measurements = vec![detect, tree];
    let summary = format_measurements(&measurements);
    let elapsed = total(&measurements);
    assert!(
        elapsed <= COLD_START_BUDGET,
        "cold start blew the budget: {summary} (budget {} ms)",
        COLD_START_BUDGET.as_millis(),
    );
    // Use detection so the optimiser doesn't drop the work.
    assert!(detection.detected(), "fixture should detect signals");
}
