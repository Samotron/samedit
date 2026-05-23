//! Cold-start benchmark harness — v0.6 M6.1.
//!
//! Headless because the v0.6 budget is "first interactive paint ≤
//! 100ms" — we measure the work cockpit does *before* the window
//! opens, and the window itself is the M6.2 splash-then-hydrate
//! frame. A real GL context is irrelevant to this number.
//!
//! Two entry points:
//!
//! * [`measure_phase`] times a single closure and returns a
//!   [`PhaseMeasurement`] callers can assert on.
//! * [`measure_cold_start`] runs a full project-open simulation
//!   (detect + tree load + model construction) and returns the per-
//!   phase breakdown.
//!
//! Avoids `criterion` to keep the dependency footprint small —
//! `Instant` math is enough for the budget assertions the CI leg
//! cares about. Switching to `criterion` later is a matter of taste.

use std::time::{Duration, Instant};

/// One phase of work and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseMeasurement {
    pub phase: &'static str,
    pub elapsed: Duration,
}

impl PhaseMeasurement {
    /// True when `elapsed` stays under `budget`. Status-line helper.
    pub fn within(&self, budget: Duration) -> bool {
        self.elapsed <= budget
    }
}

/// Run `work` and return the measured duration.
pub fn measure_phase<R>(phase: &'static str, work: impl FnOnce() -> R) -> (R, PhaseMeasurement) {
    let start = Instant::now();
    let value = work();
    let elapsed = start.elapsed();
    (value, PhaseMeasurement { phase, elapsed })
}

/// Render a list of measurements as one human-readable line.
pub fn format_measurements(measurements: &[PhaseMeasurement]) -> String {
    let total: Duration = measurements.iter().map(|m| m.elapsed).sum();
    let mut text = format!("cold start total {} ms — ", total.as_millis());
    for (i, m) in measurements.iter().enumerate() {
        if i > 0 {
            text.push_str(" · ");
        }
        text.push_str(m.phase);
        text.push('=');
        text.push_str(&format!("{} ms", m.elapsed.as_millis()));
    }
    text
}

/// Sum every measurement's duration — convenient for budget checks.
pub fn total(measurements: &[PhaseMeasurement]) -> Duration {
    measurements.iter().map(|m| m.elapsed).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn measure_phase_returns_the_closure_value_and_its_duration() {
        let (value, measurement) = measure_phase("startup.test.identity", || 42);
        assert_eq!(value, 42);
        assert_eq!(measurement.phase, "startup.test.identity");
        assert!(measurement.elapsed >= Duration::ZERO);
    }

    #[test]
    fn within_compares_against_a_budget() {
        let (_, m) = measure_phase("startup.test.tiny", || sleep(Duration::from_micros(1)));
        assert!(m.within(Duration::from_secs(1)));
    }

    #[test]
    fn format_measurements_includes_per_phase_and_total() {
        let measurements = vec![
            PhaseMeasurement {
                phase: "startup.detect",
                elapsed: Duration::from_millis(5),
            },
            PhaseMeasurement {
                phase: "startup.tree",
                elapsed: Duration::from_millis(10),
            },
        ];
        let text = format_measurements(&measurements);
        assert!(text.contains("total 15 ms"));
        assert!(text.contains("startup.detect=5 ms"));
        assert!(text.contains("startup.tree=10 ms"));
    }

    #[test]
    fn total_sums_every_measurement() {
        let measurements = vec![
            PhaseMeasurement {
                phase: "a",
                elapsed: Duration::from_millis(3),
            },
            PhaseMeasurement {
                phase: "b",
                elapsed: Duration::from_millis(7),
            },
        ];
        assert_eq!(total(&measurements), Duration::from_millis(10));
    }
}
