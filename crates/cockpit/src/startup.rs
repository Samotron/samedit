//! Startup tracing — v0.6 M6.7.
//!
//! Records every cold-start phase (`startup.detect`, `startup.tree`,
//! `startup.model`, `startup.cache`, `startup.git`, `startup.window`)
//! with wall-clock durations so the `Debug: Show Startup Trace`
//! command and the M6.1 benchmark harness have one source of truth.
//!
//! Spans land in `tracing` so `RUST_LOG=cockpit=trace` (or
//! `COCKPIT_LOG=trace`) surfaces the same data; the in-process record
//! is what the debug command reads back without re-running startup.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Single shared recorder. We use a `Mutex` rather than a thread-local
/// so the binary's event loop (which runs on the main thread) and any
/// future background warmup threads all write into the same trace.
static TRACE: Mutex<StartupTrace> = Mutex::new(StartupTrace::new());

/// Time the closure, record it under `phase`, and propagate its result.
pub fn time_phase<R>(phase: &'static str, work: impl FnOnce() -> R) -> R {
    let span = tracing::info_span!("startup", phase);
    let _guard = span.enter();
    let start = Instant::now();
    let value = work();
    let elapsed = start.elapsed();
    record(phase, elapsed);
    tracing::info!(
        phase,
        elapsed_us = elapsed.as_micros() as u64,
        "startup.phase"
    );
    value
}

/// Append a pre-measured phase entry — for callbacks that already own
/// the timing (e.g. the windowing harness reporting frame 1 paint).
pub fn record(phase: &'static str, elapsed: Duration) {
    if let Ok(mut trace) = TRACE.lock() {
        trace.push(phase, elapsed);
    }
}

/// Snapshot the recorded entries — the debug command formats this for
/// the status line. Cheap (one lock, one clone).
pub fn snapshot() -> Vec<PhaseEntry> {
    TRACE
        .lock()
        .map(|trace| trace.entries.clone())
        .unwrap_or_default()
}

/// One recorded phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseEntry {
    pub phase: &'static str,
    pub elapsed_us: u64,
}

/// Format a snapshot for the status line — newest entry last, total
/// time at the head so `Debug: Show Startup Trace` answers "how slow
/// was the launch?" without scrolling.
pub fn format_snapshot(entries: &[PhaseEntry]) -> String {
    if entries.is_empty() {
        return "startup: (no phases recorded)".to_string();
    }
    let total: u64 = entries.iter().map(|e| e.elapsed_us).sum();
    let mut text = format!("startup total {} ms — ", total / 1000);
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            text.push_str(" · ");
        }
        text.push_str(entry.phase);
        text.push('=');
        text.push_str(&format!("{} ms", entry.elapsed_us / 1000));
    }
    text
}

#[derive(Debug)]
struct StartupTrace {
    entries: Vec<PhaseEntry>,
}

impl StartupTrace {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn push(&mut self, phase: &'static str, elapsed: Duration) {
        self.entries.push(PhaseEntry {
            phase,
            elapsed_us: elapsed.as_micros() as u64,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_phase_records_an_entry_with_the_right_label() {
        // Run inside a unique label so concurrent test runs don't
        // smear results into the global trace.
        time_phase("startup.test.basic", || {});
        let snap = snapshot();
        assert!(snap.iter().any(|e| e.phase == "startup.test.basic"));
    }

    #[test]
    fn format_snapshot_includes_the_total() {
        let entries = vec![
            PhaseEntry {
                phase: "startup.detect",
                elapsed_us: 5_000,
            },
            PhaseEntry {
                phase: "startup.tree",
                elapsed_us: 10_000,
            },
        ];
        let text = format_snapshot(&entries);
        assert!(text.contains("total 15 ms"));
        assert!(text.contains("startup.detect=5 ms"));
        assert!(text.contains("startup.tree=10 ms"));
    }

    #[test]
    fn format_snapshot_handles_empty_input() {
        assert!(format_snapshot(&[]).contains("no phases recorded"));
    }
}
