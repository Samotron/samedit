//! Crew review view-model (v0.14 M14.4).
//!
//! The headless view-model for the agent-crew pane: a list of candidate
//! agents with their status and diffstat, a keyboard focus cursor, and a
//! single [`CrewIntent`] output the binary lowers onto the `crew.*` commands
//! (real git / PTY I/O). It wraps a live [`cockpit_crew::CrewRun`] — the
//! state machine stays the source of truth; this layer only adds focus and
//! intent. Pure data, no window, no git (spec §18.8).

use std::path::PathBuf;

use cockpit_crew::{AgentId, AgentState, CrewRun, DiffStat, RunId, RunOutcome};

/// A UI-facing flattening of [`AgentState`] — the renderer only cares about
/// the badge and label, not the embedded diff/error payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Pending,
    Preparing,
    Running,
    Succeeded,
    Failed,
    Picked,
    Discarded,
}

impl AgentStatus {
    fn from_state(state: &AgentState) -> Self {
        match state {
            AgentState::Pending => Self::Pending,
            AgentState::Preparing => Self::Preparing,
            AgentState::Running => Self::Running,
            AgentState::Succeeded(_) => Self::Succeeded,
            AgentState::Failed(_) => Self::Failed,
            AgentState::Picked => Self::Picked,
            AgentState::Discarded => Self::Discarded,
        }
    }

    /// A single-character badge for the candidate list.
    pub fn badge(self) -> char {
        match self {
            Self::Pending => '·',
            Self::Preparing => '⋯',
            Self::Running => '▶',
            Self::Succeeded => '✓',
            Self::Failed => '✗',
            Self::Picked => '★',
            Self::Discarded => '–',
        }
    }

    /// Human-readable status label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Preparing => "preparing",
            Self::Running => "running",
            Self::Succeeded => "done",
            Self::Failed => "failed",
            Self::Picked => "picked",
            Self::Discarded => "discarded",
        }
    }
}

/// One row in the candidate list, as the UI renders it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewAgentRow {
    pub id: AgentId,
    pub name: String,
    pub branch: String,
    pub worktree: PathBuf,
    pub status: AgentStatus,
    /// Captured diff once the agent has finished, for the `+/−` badge.
    pub diff: Option<DiffStat>,
    /// Failure message when `status == Failed`.
    pub error: Option<String>,
    /// True for the row with the keyboard cursor.
    pub focused: bool,
}

/// Action the user requested in the crew pane; the binary turns it into the
/// matching `crew.*` command + real I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewIntent {
    /// Integrate this agent's worktree and discard the rest.
    Pick(AgentId),
    /// Drop this agent and prune its worktree.
    Discard(AgentId),
    /// Re-run this agent in a fresh worktree.
    Retry(AgentId),
    /// Open this agent's diff against the base in the editor.
    OpenDiff(AgentId),
    /// Attach a terminal to this agent's worktree.
    OpenTerminal(AgentId),
    /// Cancel the whole run.
    Cancel(RunId),
}

/// Header counts for the crew pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrewSummary {
    pub total: usize,
    pub active: usize,
    pub reviewable: usize,
    pub settled: bool,
    pub outcome: RunOutcome,
}

/// The crew review pane: a [`CrewRun`] plus a focus cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewView {
    run: CrewRun,
    focus: usize,
}

impl CrewView {
    /// Wrap a run; focus starts on the first agent.
    pub fn new(run: CrewRun) -> Self {
        Self { run, focus: 0 }
    }

    /// Borrow the underlying run (read-only — the orchestrator owns mutation).
    pub fn run(&self) -> &CrewRun {
        &self.run
    }

    /// Mutable access for the orchestrator to drive agent lifecycle, after
    /// which [`CrewView`] re-derives its rows. Focus is clamped on read.
    pub fn run_mut(&mut self) -> &mut CrewRun {
        &mut self.run
    }

    /// The candidate rows in run order, with the focused flag set.
    pub fn rows(&self) -> Vec<CrewAgentRow> {
        let focus = self.clamped_focus();
        self.run
            .agents()
            .iter()
            .enumerate()
            .map(|(i, agent)| CrewAgentRow {
                id: agent.id(),
                name: agent.agent().to_string(),
                branch: agent.branch().to_string(),
                worktree: agent.worktree().clone(),
                status: AgentStatus::from_state(agent.state()),
                diff: agent.diff_stat(),
                error: match agent.state() {
                    AgentState::Failed(msg) => Some(msg.clone()),
                    _ => None,
                },
                focused: i == focus,
            })
            .collect()
    }

    /// Header counts for the pane.
    pub fn summary(&self) -> CrewSummary {
        CrewSummary {
            total: self.run.agents().len(),
            active: self.run.active_count(),
            reviewable: self.run.reviewable_count(),
            settled: self.run.all_settled(),
            outcome: self.run.outcome(),
        }
    }

    /// Index of the focused row (clamped into range; 0 when empty).
    pub fn focus(&self) -> usize {
        self.clamped_focus()
    }

    /// Id of the focused agent, if any.
    pub fn focused_agent(&self) -> Option<AgentId> {
        self.run.agents().get(self.clamped_focus()).map(|a| a.id())
    }

    /// Move focus to the next candidate, wrapping at the end.
    pub fn focus_next(&mut self) {
        let len = self.run.agents().len();
        if len == 0 {
            return;
        }
        self.focus = (self.clamped_focus() + 1) % len;
    }

    /// Move focus to the previous candidate, wrapping at the start.
    pub fn focus_previous(&mut self) {
        let len = self.run.agents().len();
        if len == 0 {
            return;
        }
        self.focus = (self.clamped_focus() + len - 1) % len;
    }

    /// Focus a specific agent by id. No-op if it isn't in the run.
    pub fn focus_agent(&mut self, id: AgentId) {
        if let Some(i) = self.run.agents().iter().position(|a| a.id() == id) {
            self.focus = i;
        }
    }

    /// Emit a [`CrewIntent::Pick`] for the focused agent — only when it is
    /// reviewable (succeeded). Returns `None` otherwise so the binary doesn't
    /// dispatch a doomed command.
    pub fn pick_focused(&self) -> Option<CrewIntent> {
        let agent = self.run.agents().get(self.clamped_focus())?;
        agent
            .state()
            .is_reviewable()
            .then(|| CrewIntent::Pick(agent.id()))
    }

    /// Emit a [`CrewIntent::Discard`] for the focused agent, unless it's
    /// already final (picked/discarded).
    pub fn discard_focused(&self) -> Option<CrewIntent> {
        let agent = self.run.agents().get(self.clamped_focus())?;
        (!agent.state().is_final()).then(|| CrewIntent::Discard(agent.id()))
    }

    /// Emit a [`CrewIntent::Retry`] for the focused agent — only when it
    /// finished unfavourably (failed or discarded) and can be re-run.
    pub fn retry_focused(&self) -> Option<CrewIntent> {
        let agent = self.run.agents().get(self.clamped_focus())?;
        matches!(agent.state(), AgentState::Failed(_) | AgentState::Discarded)
            .then(|| CrewIntent::Retry(agent.id()))
    }

    /// Emit a [`CrewIntent::OpenDiff`] for the focused agent — only when it
    /// has a diff to show.
    pub fn open_diff_focused(&self) -> Option<CrewIntent> {
        let agent = self.run.agents().get(self.clamped_focus())?;
        agent
            .diff_stat()
            .is_some()
            .then(|| CrewIntent::OpenDiff(agent.id()))
    }

    /// Emit a [`CrewIntent::OpenTerminal`] for the focused agent's worktree.
    pub fn open_terminal_focused(&self) -> Option<CrewIntent> {
        self.focused_agent().map(CrewIntent::OpenTerminal)
    }

    /// Emit a [`CrewIntent::Cancel`] for the run.
    pub fn cancel(&self) -> CrewIntent {
        CrewIntent::Cancel(self.run.id())
    }

    fn clamped_focus(&self) -> usize {
        let len = self.run.agents().len();
        if len == 0 { 0 } else { self.focus.min(len - 1) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_crew::{AgentSpec, CrewPlan, CrewTask};

    fn view(n: usize) -> CrewView {
        let agents = (0..n)
            .map(|i| {
                AgentSpec::new(
                    format!("agent{i}"),
                    "claude",
                    vec!["-p".into(), "{prompt}".into()],
                )
            })
            .collect();
        let plan = CrewPlan {
            task: CrewTask::new("speed it up"),
            agents,
            worktree_root: PathBuf::from("/cache/crew"),
            branch_prefix: "crew".into(),
        };
        CrewView::new(plan.materialise(RunId::new(1)))
    }

    fn drive_to_succeeded(view: &mut CrewView, idx: usize, diff: DiffStat) {
        let id = view.run().agents()[idx].id();
        let run = view.run_mut();
        let a = run.agent_mut(id).unwrap();
        a.start_preparing().unwrap();
        a.mark_running().unwrap();
        a.succeed(diff).unwrap();
    }

    #[test]
    fn rows_track_focus_and_state() {
        let mut view = view(3);
        let rows = view.rows();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].focused);
        assert_eq!(rows[0].status, AgentStatus::Pending);
        assert_eq!(rows[0].name, "agent0");

        view.focus_next();
        assert_eq!(view.focus(), 1);
        assert!(view.rows()[1].focused);

        view.focus_previous();
        view.focus_previous();
        // wraps from 0 → last
        assert_eq!(view.focus(), 2);
    }

    #[test]
    fn diffstat_and_status_surface_on_rows() {
        let mut view = view(2);
        drive_to_succeeded(
            &mut view,
            0,
            DiffStat {
                files_changed: 3,
                insertions: 12,
                deletions: 4,
            },
        );
        let rows = view.rows();
        assert_eq!(rows[0].status, AgentStatus::Succeeded);
        assert_eq!(rows[0].diff.unwrap().insertions, 12);
        assert_eq!(rows[0].status.badge(), '✓');
    }

    #[test]
    fn failure_message_surfaces_on_row() {
        let mut view = view(1);
        let id = view.run().agents()[0].id();
        {
            let run = view.run_mut();
            let a = run.agent_mut(id).unwrap();
            a.start_preparing().unwrap();
            a.fail("model timed out").unwrap();
        }
        let row = &view.rows()[0];
        assert_eq!(row.status, AgentStatus::Failed);
        assert_eq!(row.error.as_deref(), Some("model timed out"));
    }

    #[test]
    fn pick_intent_only_when_reviewable() {
        let mut view = view(2);
        // Nothing reviewable yet → no pick intent.
        assert_eq!(view.pick_focused(), None);

        drive_to_succeeded(&mut view, 0, DiffStat::default());
        let id0 = view.run().agents()[0].id();
        assert_eq!(view.pick_focused(), Some(CrewIntent::Pick(id0)));
    }

    #[test]
    fn retry_intent_only_for_failed_or_discarded() {
        let mut view = view(1);
        assert_eq!(view.retry_focused(), None); // pending
        let id = view.run().agents()[0].id();
        {
            let run = view.run_mut();
            let a = run.agent_mut(id).unwrap();
            a.start_preparing().unwrap();
            a.fail("boom").unwrap();
        }
        assert_eq!(view.retry_focused(), Some(CrewIntent::Retry(id)));
    }

    #[test]
    fn open_diff_intent_requires_a_diff() {
        let mut view = view(1);
        assert_eq!(view.open_diff_focused(), None);
        drive_to_succeeded(
            &mut view,
            0,
            DiffStat {
                files_changed: 1,
                insertions: 1,
                deletions: 0,
            },
        );
        let id = view.run().agents()[0].id();
        assert_eq!(view.open_diff_focused(), Some(CrewIntent::OpenDiff(id)));
    }

    #[test]
    fn summary_reports_counts() {
        let mut view = view(3);
        drive_to_succeeded(&mut view, 0, DiffStat::default());
        let s = view.summary();
        assert_eq!(s.total, 3);
        assert_eq!(s.reviewable, 1);
        assert_eq!(s.active, 2);
        assert!(!s.settled);
        assert_eq!(s.outcome, RunOutcome::InProgress);
    }

    #[test]
    fn focus_clamps_when_unset_and_empty_is_safe() {
        let mut view = view(0);
        assert_eq!(view.focus(), 0);
        assert_eq!(view.focused_agent(), None);
        assert_eq!(view.rows().len(), 0);
        // Navigation on an empty run is a no-op, not a panic.
        view.focus_next();
        view.focus_previous();
        assert_eq!(view.pick_focused(), None);
    }
}
