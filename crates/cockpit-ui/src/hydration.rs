//! Splash-then-hydrate progress — v0.6 M6.2.
//!
//! The cockpit opens its window before running the slow cold-start phases
//! (project detection, file-tree walk, model build, user-config load, git
//! refresh, project-cache restore). This module models that progress as a
//! pure state machine so the splash UI is fully testable without a GPU,
//! and so the binary's hydration driver has one place to record what's
//! done.
//!
//! The phase work itself lives in the binary (`cockpit/src/hydration.rs`)
//! because it owns `cockpit-project` data — keeping it out of `cockpit-ui`
//! preserves the headless contract (AGENTS §2 hard rule 1).
//!
//! Lifecycle (happy path):
//! 1. [`HydrationProgress::default_phases`] creates a queue with the six
//!    canonical phases queued.
//! 2. Each frame the driver calls [`HydrationProgress::begin_next`] to
//!    claim the next phase, runs the work, then calls
//!    [`HydrationProgress::complete_current`] with the measured duration.
//! 3. When [`HydrationProgress::is_done`] flips, the shell switches from
//!    splash painting to the live model.
//!
//! A failed phase calls [`HydrationProgress::fail`], which clears any
//! remaining work, records the error, and leaves the progress in a
//! terminal state so the splash can render the failure message.

use std::time::Duration;

/// Canonical cold-start phase reported in the splash.
///
/// Ordering matches the dependency chain of the data the next phase
/// consumes — detection feeds the file tree, the tree feeds model
/// construction, the model is then enriched by config / git / cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HydrationPhase {
    Detect,
    LoadTree,
    BuildModel,
    ApplyConfig,
    RefreshGit,
    RestoreCache,
}

impl HydrationPhase {
    /// The full canonical phase queue.
    pub const ALL: [HydrationPhase; 6] = [
        HydrationPhase::Detect,
        HydrationPhase::LoadTree,
        HydrationPhase::BuildModel,
        HydrationPhase::ApplyConfig,
        HydrationPhase::RefreshGit,
        HydrationPhase::RestoreCache,
    ];

    /// Short human label rendered on the splash next to the spinner.
    pub fn label(self) -> &'static str {
        match self {
            HydrationPhase::Detect => "Detecting project",
            HydrationPhase::LoadTree => "Loading file tree",
            HydrationPhase::BuildModel => "Preparing workspace",
            HydrationPhase::ApplyConfig => "Loading user config",
            HydrationPhase::RefreshGit => "Reading git status",
            HydrationPhase::RestoreCache => "Restoring last session",
        }
    }

    /// Tracing span name. Lines up with the spans recorded by
    /// [`crate::startup::time_phase`] so the post-hoc startup trace shows
    /// the same labels as live `tracing` output.
    pub fn span(self) -> &'static str {
        match self {
            HydrationPhase::Detect => "startup.detect",
            HydrationPhase::LoadTree => "startup.tree",
            HydrationPhase::BuildModel => "startup.model",
            HydrationPhase::ApplyConfig => "startup.config",
            HydrationPhase::RefreshGit => "startup.git",
            HydrationPhase::RestoreCache => "startup.cache",
        }
    }
}

/// One phase that has finished, with its measured wall-clock duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletedPhase {
    pub phase: HydrationPhase,
    pub elapsed_us: u64,
}

/// Splash progress state. Pure data; safe to share with the painter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HydrationProgress {
    pending: Vec<HydrationPhase>,
    current: Option<HydrationPhase>,
    completed: Vec<CompletedPhase>,
    error: Option<String>,
}

impl HydrationProgress {
    /// Build a progress tracker over `phases`.
    pub fn new(phases: Vec<HydrationPhase>) -> Self {
        Self {
            pending: phases,
            ..Default::default()
        }
    }

    /// The canonical queue of all six phases.
    pub fn default_phases() -> Self {
        Self::new(HydrationPhase::ALL.to_vec())
    }

    /// Phases still to run, in order.
    pub fn pending(&self) -> &[HydrationPhase] {
        &self.pending
    }

    /// The phase currently being run, if any.
    pub fn current(&self) -> Option<HydrationPhase> {
        self.current
    }

    /// Phases that have finished, oldest first.
    pub fn completed(&self) -> &[CompletedPhase] {
        &self.completed
    }

    /// Error recorded by [`HydrationProgress::fail`], if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// True once every phase has finished and no error was recorded.
    pub fn is_done(&self) -> bool {
        self.error.is_none() && self.current.is_none() && self.pending.is_empty()
    }

    /// True if hydration ended in failure.
    pub fn is_failed(&self) -> bool {
        self.error.is_some()
    }

    /// Claim the next pending phase as `current` and hand it back to the
    /// caller to run. Returns `None` if hydration is finished, failed, or
    /// already running a phase.
    pub fn begin_next(&mut self) -> Option<HydrationPhase> {
        if self.error.is_some() || self.current.is_some() {
            return None;
        }
        if self.pending.is_empty() {
            return None;
        }
        let phase = self.pending.remove(0);
        self.current = Some(phase);
        Some(phase)
    }

    /// Mark the active phase finished with the supplied elapsed time.
    /// No-op if no phase is currently running.
    pub fn complete_current(&mut self, elapsed: Duration) {
        if let Some(phase) = self.current.take() {
            self.completed.push(CompletedPhase {
                phase,
                elapsed_us: elapsed.as_micros() as u64,
            });
        }
    }

    /// Record a fatal phase failure. Subsequent calls to
    /// [`HydrationProgress::begin_next`] return `None`.
    pub fn fail(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.current = None;
        self.pending.clear();
    }

    /// Fraction of phases finished, in `0.0..=1.0`. An empty progress
    /// (no phases queued, no error) reports `1.0`.
    pub fn fraction(&self) -> f32 {
        let pending = self.pending.len();
        let running = usize::from(self.current.is_some());
        let total = self.completed.len() + pending + running;
        if total == 0 {
            return 1.0;
        }
        self.completed.len() as f32 / total as f32
    }

    /// Label suitable for the splash: the running phase's name, the error
    /// message, "ready", or "starting" before the first phase begins.
    pub fn current_label(&self) -> &str {
        if let Some(message) = self.error.as_deref() {
            return message;
        }
        if let Some(phase) = self.current {
            return phase.label();
        }
        if self.is_done() {
            return "Ready";
        }
        "Starting"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_phases_queues_every_canonical_phase_in_order() {
        let progress = HydrationProgress::default_phases();
        assert_eq!(progress.pending(), HydrationPhase::ALL);
        assert!(progress.current().is_none());
        assert!(progress.completed().is_empty());
        assert!(!progress.is_done(), "freshly queued progress is not done");
    }

    #[test]
    fn begin_next_consumes_phases_in_order_until_drained() {
        let mut progress = HydrationProgress::default_phases();
        for expected in HydrationPhase::ALL {
            assert_eq!(progress.begin_next(), Some(expected));
            progress.complete_current(Duration::from_micros(123));
        }
        assert!(progress.begin_next().is_none(), "queue should be empty");
        assert!(progress.is_done());
        assert_eq!(progress.fraction(), 1.0);
    }

    #[test]
    fn begin_next_refuses_to_double_book_a_phase() {
        let mut progress = HydrationProgress::default_phases();
        assert!(progress.begin_next().is_some());
        assert!(
            progress.begin_next().is_none(),
            "must not start a second phase while one is running",
        );
    }

    #[test]
    fn fail_freezes_progress_and_records_the_error() {
        let mut progress = HydrationProgress::default_phases();
        progress.begin_next();
        progress.fail("detect crashed");

        assert!(progress.is_failed());
        assert!(progress.error().is_some());
        assert!(progress.current().is_none());
        assert!(progress.pending().is_empty());
        assert!(
            progress.begin_next().is_none(),
            "failed progress is terminal"
        );
        assert!(
            !progress.is_done(),
            "is_done is the happy-path predicate; failed != done",
        );
    }

    #[test]
    fn fraction_reflects_completed_share_of_the_queue() {
        let mut progress = HydrationProgress::default_phases();
        assert_eq!(progress.fraction(), 0.0);
        progress.begin_next();
        progress.complete_current(Duration::from_micros(50));
        assert!((progress.fraction() - 1.0 / 6.0).abs() < 1e-6);
    }

    #[test]
    fn current_label_reports_the_active_phase_then_falls_back() {
        let mut progress = HydrationProgress::default_phases();
        assert_eq!(progress.current_label(), "Starting");

        progress.begin_next();
        assert_eq!(progress.current_label(), HydrationPhase::Detect.label());

        for _ in 0..HydrationPhase::ALL.len() {
            progress.complete_current(Duration::from_micros(10));
            progress.begin_next();
        }
        progress.complete_current(Duration::from_micros(10));
        assert_eq!(progress.current_label(), "Ready");
    }

    #[test]
    fn current_label_surfaces_the_failure_message() {
        let mut progress = HydrationProgress::default_phases();
        progress.begin_next();
        progress.fail("detect: permission denied");
        assert_eq!(progress.current_label(), "detect: permission denied");
    }

    #[test]
    fn each_phase_has_a_distinct_tracing_span_name() {
        let mut spans: Vec<&'static str> = HydrationPhase::ALL.iter().map(|p| p.span()).collect();
        let count = spans.len();
        spans.sort();
        spans.dedup();
        assert_eq!(spans.len(), count, "spans must be unique");
    }
}
