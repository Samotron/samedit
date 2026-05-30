//! The crew run model — one task fanned out to N agents, each in its own
//! isolated worktree, plus the pick/discard review state machine.
//!
//! This is the pure brain of v0.14: no git, no PTY, no clock. The
//! [`crate::worktree`] module owns the git side; [`crate::spec`] turns config
//! into a [`CrewRun`]. Every state change here is a guarded transition that
//! returns a [`CrewError`] rather than silently no-op'ing, so the UI and the
//! command layer can surface "you can't pick a failed agent" instead of
//! leaving the user wondering why nothing happened.

use std::path::PathBuf;

use cockpit_project::env::ProcessSpec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Stable identifier for a crew run (one task fanned out to several agents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(u64);

impl RunId {
    /// Wrap a raw id. The allocator that hands these out lives in the binary.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying value.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Stable identifier for one agent within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AgentId(u64);

impl AgentId {
    /// Wrap a raw id.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying value.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What the worktrees are branched off. Lowered to a git revision by
/// [`BaseRef::as_git_ref`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseRef {
    /// The current `HEAD`.
    Head,
    /// A named branch.
    Branch(String),
    /// A specific commit (sha or any rev-parse-able string).
    Commit(String),
}

impl BaseRef {
    /// The revision string handed to `git worktree add` / `git diff`.
    pub fn as_git_ref(&self) -> &str {
        match self {
            BaseRef::Head => "HEAD",
            BaseRef::Branch(name) => name,
            BaseRef::Commit(sha) => sha,
        }
    }
}

/// The shared task every agent in a run is asked to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewTask {
    /// The prompt / instruction handed to each agent.
    pub prompt: String,
    /// The revision the worktrees branch off.
    pub base: BaseRef,
}

impl CrewTask {
    /// A task off the current `HEAD`.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            base: BaseRef::Head,
        }
    }

    /// Set an explicit base revision.
    pub fn base(mut self, base: BaseRef) -> Self {
        self.base = base;
        self
    }
}

/// Summary of an agent worktree's diff against the run's base. Parsed from
/// `git diff --numstat` (see [`crate::worktree`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffStat {
    /// Number of files touched.
    pub files_changed: u32,
    /// Lines added.
    pub insertions: u32,
    /// Lines removed.
    pub deletions: u32,
}

impl DiffStat {
    /// True when the agent produced no change at all — a useful signal that
    /// the agent gave up or refused the task.
    pub fn is_empty(&self) -> bool {
        self.files_changed == 0 && self.insertions == 0 && self.deletions == 0
    }
}

/// Lifecycle of one agent within a run.
///
/// ```text
///  Pending ─▶ Preparing ─▶ Running ─┬─▶ Succeeded(diff) ─┬─▶ Picked
///                  │           │      └─▶ Failed(msg)      └─▶ Discarded
///                  └───────────┴────────────────────────────▶ Failed(msg)
/// ```
///
/// `Picked` and `Discarded` are terminal; everything else can still move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Queued; the worktree has not been created yet.
    Pending,
    /// Worktree created; the agent process is being spawned.
    Preparing,
    /// The agent process is running in its worktree.
    Running,
    /// The agent exited cleanly; its diff against the base is captured.
    Succeeded(DiffStat),
    /// The agent failed to spawn or exited non-zero.
    Failed(String),
    /// Chosen as the winner — its worktree is the one to integrate.
    Picked,
    /// Dropped; its worktree should be pruned.
    Discarded,
}

impl AgentState {
    /// A short, stable label for diagnostics and transition errors.
    pub fn label(&self) -> &'static str {
        match self {
            AgentState::Pending => "pending",
            AgentState::Preparing => "preparing",
            AgentState::Running => "running",
            AgentState::Succeeded(_) => "succeeded",
            AgentState::Failed(_) => "failed",
            AgentState::Picked => "picked",
            AgentState::Discarded => "discarded",
        }
    }

    /// True while the agent still has work or a process in flight.
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            AgentState::Pending | AgentState::Preparing | AgentState::Running
        )
    }

    /// True once the agent can no longer change — picked or discarded.
    pub fn is_final(&self) -> bool {
        matches!(self, AgentState::Picked | AgentState::Discarded)
    }

    /// True when the agent finished cleanly and has a diff to review.
    pub fn is_reviewable(&self) -> bool {
        matches!(self, AgentState::Succeeded(_))
    }
}

/// One agent's slot in a run: its identity, its isolated worktree, the
/// command that drives it, and its current [`AgentState`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRun {
    id: AgentId,
    agent: String,
    branch: String,
    worktree: PathBuf,
    command: ProcessSpec,
    state: AgentState,
    /// The captured diff, retained once the agent succeeds so it survives the
    /// terminal `Picked`/`Discarded` transition (a reviewer still wants to
    /// see which candidate they kept).
    diff: Option<DiffStat>,
}

impl AgentRun {
    /// Build a fresh, `Pending` agent run.
    pub fn new(
        id: AgentId,
        agent: impl Into<String>,
        branch: impl Into<String>,
        worktree: impl Into<PathBuf>,
        command: ProcessSpec,
    ) -> Self {
        Self {
            id,
            agent: agent.into(),
            branch: branch.into(),
            worktree: worktree.into(),
            command,
            state: AgentState::Pending,
            diff: None,
        }
    }

    /// Agent id within the run.
    pub fn id(&self) -> AgentId {
        self.id
    }

    /// Configured agent name (e.g. `claude`, `codex`).
    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// The branch its worktree is checked out on.
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Absolute path to the agent's isolated worktree.
    pub fn worktree(&self) -> &PathBuf {
        &self.worktree
    }

    /// The resolved command that drives the agent inside its worktree.
    pub fn command(&self) -> &ProcessSpec {
        &self.command
    }

    /// Current lifecycle state.
    pub fn state(&self) -> &AgentState {
        &self.state
    }

    /// The captured diff, once the agent has succeeded. Retained through the
    /// terminal `Picked`/`Discarded` transition.
    pub fn diff_stat(&self) -> Option<DiffStat> {
        self.diff
    }

    /// `Pending → Preparing`: the worktree is being created.
    pub fn start_preparing(&mut self) -> Result<(), CrewError> {
        self.transition(
            matches!(self.state, AgentState::Pending),
            AgentState::Preparing,
        )
    }

    /// `Preparing → Running`: the agent process is live.
    pub fn mark_running(&mut self) -> Result<(), CrewError> {
        self.transition(
            matches!(self.state, AgentState::Preparing),
            AgentState::Running,
        )
    }

    /// `Running → Succeeded(diff)`: clean exit with a captured diff. The diff
    /// is also stashed on the agent so it outlives a later pick/discard.
    pub fn succeed(&mut self, diff: DiffStat) -> Result<(), CrewError> {
        self.transition(
            matches!(self.state, AgentState::Running),
            AgentState::Succeeded(diff),
        )?;
        self.diff = Some(diff);
        Ok(())
    }

    /// `Preparing | Running → Failed(msg)`: spawn error or non-zero exit.
    pub fn fail(&mut self, message: impl Into<String>) -> Result<(), CrewError> {
        self.transition(
            matches!(self.state, AgentState::Preparing | AgentState::Running),
            AgentState::Failed(message.into()),
        )
    }

    /// Internal: discard from any non-final state. Idempotent on already-final
    /// states is *not* allowed — the caller (`CrewRun`) only discards live or
    /// reviewable agents.
    fn discard(&mut self) -> Result<(), CrewError> {
        self.transition(!self.state.is_final(), AgentState::Discarded)
    }

    /// Internal: mark the winner. Only a reviewable (succeeded) agent.
    fn pick(&mut self) -> Result<(), CrewError> {
        self.transition(self.state.is_reviewable(), AgentState::Picked)
    }

    /// Internal: reset a finished-but-unpicked agent (`Failed`/`Discarded`)
    /// back to `Pending` for a fresh attempt, swapping in a new
    /// branch/worktree/command (the old worktree may be dirty or pruned).
    fn reset_for_retry(
        &mut self,
        branch: String,
        worktree: PathBuf,
        command: ProcessSpec,
    ) -> Result<(), CrewError> {
        match self.state {
            AgentState::Failed(_) | AgentState::Discarded => {
                self.branch = branch;
                self.worktree = worktree;
                self.command = command;
                self.diff = None;
                self.state = AgentState::Pending;
                Ok(())
            }
            _ => Err(CrewError::IllegalTransition {
                id: self.id,
                from: self.state.label(),
                to: "pending",
            }),
        }
    }

    fn transition(&mut self, allowed: bool, to: AgentState) -> Result<(), CrewError> {
        if allowed {
            self.state = to;
            Ok(())
        } else {
            Err(CrewError::IllegalTransition {
                id: self.id,
                from: self.state.label(),
                to: to.label(),
            })
        }
    }
}

/// Where a run ended up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Agents are still working or awaiting a decision.
    InProgress,
    /// A winner was picked; the rest were discarded.
    Decided(AgentId),
    /// The whole run was cancelled.
    Cancelled,
}

/// A crew run: the task, the parallel [`AgentRun`]s, and the review decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewRun {
    id: RunId,
    task: CrewTask,
    agents: Vec<AgentRun>,
    outcome: RunOutcome,
}

impl CrewRun {
    /// Assemble a run from its agents. Usually produced by
    /// [`crate::spec::CrewPlan::materialise`] rather than by hand.
    pub fn new(id: RunId, task: CrewTask, agents: Vec<AgentRun>) -> Self {
        Self {
            id,
            task,
            agents,
            outcome: RunOutcome::InProgress,
        }
    }

    /// Run id.
    pub fn id(&self) -> RunId {
        self.id
    }

    /// The shared task.
    pub fn task(&self) -> &CrewTask {
        &self.task
    }

    /// All agent slots, in materialisation order.
    pub fn agents(&self) -> &[AgentRun] {
        &self.agents
    }

    /// Where the run stands.
    pub fn outcome(&self) -> RunOutcome {
        self.outcome
    }

    /// Shared borrow of one agent.
    pub fn agent(&self, id: AgentId) -> Option<&AgentRun> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// Mutable borrow of one agent — used by the orchestrator to drive
    /// individual lifecycle transitions (`start_preparing`, `succeed`, …).
    pub fn agent_mut(&mut self, id: AgentId) -> Option<&mut AgentRun> {
        self.agents.iter_mut().find(|a| a.id == id)
    }

    /// Number of agents whose process is still in flight.
    pub fn active_count(&self) -> usize {
        self.agents.iter().filter(|a| a.state.is_active()).count()
    }

    /// Number of agents that finished cleanly and can be picked.
    pub fn reviewable_count(&self) -> usize {
        self.agents
            .iter()
            .filter(|a| a.state.is_reviewable())
            .count()
    }

    /// True once no agent has a process in flight — every slot has either
    /// succeeded, failed, or is already final. The point at which the UI can
    /// stop showing spinners and let the user pick.
    pub fn all_settled(&self) -> bool {
        self.agents.iter().all(|a| !a.state.is_active())
    }

    /// The picked agent, if a decision has been made.
    pub fn winner(&self) -> Option<&AgentRun> {
        match self.outcome {
            RunOutcome::Decided(id) => self.agent(id),
            _ => None,
        }
    }

    /// Pick `id` as the winner: it becomes `Picked` and every other agent
    /// that isn't already `Failed`/`Discarded` is discarded. The chosen
    /// agent must be reviewable (succeeded), and the run must not already be
    /// decided or cancelled.
    pub fn pick(&mut self, id: AgentId) -> Result<(), CrewError> {
        if self.outcome != RunOutcome::InProgress {
            return Err(CrewError::AlreadyDecided(self.id));
        }
        let target = self
            .agents
            .iter()
            .find(|a| a.id == id)
            .ok_or(CrewError::UnknownAgent(id))?;
        if !target.state.is_reviewable() {
            return Err(CrewError::NotReviewable {
                id,
                state: target.state.label(),
            });
        }

        for agent in &mut self.agents {
            if agent.id == id {
                agent.pick()?;
            } else if !agent.state.is_final() && !matches!(agent.state, AgentState::Failed(_)) {
                agent.discard()?;
            }
        }
        self.outcome = RunOutcome::Decided(id);
        Ok(())
    }

    /// Discard a single agent without deciding the run — e.g. the user rules
    /// one candidate out early. Fails if the run is already decided or the
    /// agent is already final.
    pub fn discard(&mut self, id: AgentId) -> Result<(), CrewError> {
        if self.outcome != RunOutcome::InProgress {
            return Err(CrewError::AlreadyDecided(self.id));
        }
        let agent = self
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(CrewError::UnknownAgent(id))?;
        agent.discard()
    }

    /// Cancel the whole run: every still-active or reviewable agent is
    /// discarded. A run that's already decided cannot be cancelled.
    pub fn cancel(&mut self) -> Result<(), CrewError> {
        match self.outcome {
            RunOutcome::Decided(_) => return Err(CrewError::AlreadyDecided(self.id)),
            RunOutcome::Cancelled => return Ok(()),
            RunOutcome::InProgress => {}
        }
        for agent in &mut self.agents {
            if !agent.state.is_final() && !matches!(agent.state, AgentState::Failed(_)) {
                agent.discard()?;
            }
        }
        self.outcome = RunOutcome::Cancelled;
        Ok(())
    }

    /// Retry a failed or discarded agent: reset it to `Pending` with a fresh
    /// branch/worktree/command so the orchestrator can re-create its worktree
    /// and re-spawn it. The run must still be in progress, and the agent must
    /// be `Failed` or `Discarded` (you can't retry one that's still working or
    /// already picked).
    pub fn retry(
        &mut self,
        id: AgentId,
        branch: impl Into<String>,
        worktree: impl Into<PathBuf>,
        command: ProcessSpec,
    ) -> Result<(), CrewError> {
        if self.outcome != RunOutcome::InProgress {
            return Err(CrewError::AlreadyDecided(self.id));
        }
        let agent = self
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or(CrewError::UnknownAgent(id))?;
        agent.reset_for_retry(branch.into(), worktree.into(), command)
    }
}

/// Errors from driving a [`CrewRun`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CrewError {
    /// A lifecycle transition that the state machine forbids.
    #[error("agent {} cannot move from `{from}` to `{to}`", id.get())]
    IllegalTransition {
        /// The agent the transition was attempted on.
        id: AgentId,
        /// State it was in.
        from: &'static str,
        /// State that was requested.
        to: &'static str,
    },
    /// Referenced an agent id that isn't part of the run.
    #[error("no agent with id {}", .0.get())]
    UnknownAgent(AgentId),
    /// Tried to pick an agent that hasn't succeeded.
    #[error("agent {} is `{state}`, not reviewable", id.get())]
    NotReviewable {
        /// The agent that was picked.
        id: AgentId,
        /// Its current state label.
        state: &'static str,
    },
    /// The run already has a winner or was cancelled.
    #[error("run {} is already decided", .0.get())]
    AlreadyDecided(RunId),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ProcessSpec {
        ProcessSpec::new("claude").arg("-p").arg("do the thing")
    }

    fn agent(id: u64) -> AgentRun {
        AgentRun::new(
            AgentId::new(id),
            "claude",
            format!("crew/r1/{id}"),
            PathBuf::from(format!("/wt/r1-{id}")),
            spec(),
        )
    }

    fn run_with(n: u64) -> CrewRun {
        let agents = (0..n).map(agent).collect();
        CrewRun::new(RunId::new(1), CrewTask::new("do the thing"), agents)
    }

    #[test]
    fn happy_path_lifecycle() {
        let mut a = agent(0);
        assert_eq!(a.state(), &AgentState::Pending);
        a.start_preparing().unwrap();
        a.mark_running().unwrap();
        assert!(a.state().is_active());
        a.succeed(DiffStat {
            files_changed: 2,
            insertions: 10,
            deletions: 3,
        })
        .unwrap();
        assert!(a.state().is_reviewable());
        assert_eq!(a.diff_stat().unwrap().insertions, 10);
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut a = agent(0);
        // Can't run before preparing.
        assert!(matches!(
            a.mark_running(),
            Err(CrewError::IllegalTransition { .. })
        ));
        // Can't succeed before running.
        assert!(a.succeed(DiffStat::default()).is_err());
        // Can't fail a pending agent.
        assert!(a.fail("boom").is_err());
    }

    #[test]
    fn pick_marks_winner_and_discards_the_rest() {
        let mut run = run_with(3);
        // Drive all three to succeeded.
        for id in 0..3 {
            let a = run.agent_mut(AgentId::new(id)).unwrap();
            a.start_preparing().unwrap();
            a.mark_running().unwrap();
            a.succeed(DiffStat {
                files_changed: 1,
                insertions: id as u32,
                deletions: 0,
            })
            .unwrap();
        }
        assert!(run.all_settled());
        assert_eq!(run.reviewable_count(), 3);

        run.pick(AgentId::new(1)).unwrap();
        assert_eq!(run.outcome(), RunOutcome::Decided(AgentId::new(1)));
        assert_eq!(run.winner().unwrap().id(), AgentId::new(1));
        assert_eq!(
            run.agent(AgentId::new(0)).unwrap().state(),
            &AgentState::Discarded
        );
        assert_eq!(
            run.agent(AgentId::new(1)).unwrap().state(),
            &AgentState::Picked
        );
        assert_eq!(
            run.agent(AgentId::new(2)).unwrap().state(),
            &AgentState::Discarded
        );
    }

    #[test]
    fn picked_agent_retains_its_diff() {
        let mut run = run_with(1);
        let a = run.agent_mut(AgentId::new(0)).unwrap();
        a.start_preparing().unwrap();
        a.mark_running().unwrap();
        a.succeed(DiffStat {
            files_changed: 4,
            insertions: 20,
            deletions: 5,
        })
        .unwrap();
        run.pick(AgentId::new(0)).unwrap();
        let winner = run.winner().unwrap();
        assert_eq!(winner.state(), &AgentState::Picked);
        // The diff outlives the terminal transition.
        assert_eq!(winner.diff_stat().unwrap().insertions, 20);
    }

    #[test]
    fn pick_preserves_failed_agents_as_failed() {
        let mut run = run_with(2);
        let a0 = run.agent_mut(AgentId::new(0)).unwrap();
        a0.start_preparing().unwrap();
        a0.mark_running().unwrap();
        a0.succeed(DiffStat::default()).unwrap();
        let a1 = run.agent_mut(AgentId::new(1)).unwrap();
        a1.start_preparing().unwrap();
        a1.fail("agent crashed").unwrap();

        run.pick(AgentId::new(0)).unwrap();
        // The failed agent stays failed — not retroactively "discarded".
        assert!(matches!(
            run.agent(AgentId::new(1)).unwrap().state(),
            AgentState::Failed(_)
        ));
    }

    #[test]
    fn cannot_pick_a_failed_or_running_agent() {
        let mut run = run_with(2);
        let a0 = run.agent_mut(AgentId::new(0)).unwrap();
        a0.start_preparing().unwrap();
        a0.fail("nope").unwrap();
        assert!(matches!(
            run.pick(AgentId::new(0)),
            Err(CrewError::NotReviewable { .. })
        ));

        let a1 = run.agent_mut(AgentId::new(1)).unwrap();
        a1.start_preparing().unwrap();
        a1.mark_running().unwrap();
        assert!(matches!(
            run.pick(AgentId::new(1)),
            Err(CrewError::NotReviewable { .. })
        ));
    }

    #[test]
    fn cannot_pick_twice() {
        let mut run = run_with(2);
        for id in 0..2 {
            let a = run.agent_mut(AgentId::new(id)).unwrap();
            a.start_preparing().unwrap();
            a.mark_running().unwrap();
            a.succeed(DiffStat::default()).unwrap();
        }
        run.pick(AgentId::new(0)).unwrap();
        assert!(matches!(
            run.pick(AgentId::new(1)),
            Err(CrewError::AlreadyDecided(_))
        ));
    }

    #[test]
    fn unknown_agent_is_an_error() {
        let mut run = run_with(1);
        assert!(matches!(
            run.pick(AgentId::new(99)),
            Err(CrewError::UnknownAgent(_))
        ));
        assert!(matches!(
            run.discard(AgentId::new(99)),
            Err(CrewError::UnknownAgent(_))
        ));
    }

    #[test]
    fn cancel_discards_live_agents_and_keeps_failures() {
        let mut run = run_with(3);
        let a0 = run.agent_mut(AgentId::new(0)).unwrap();
        a0.start_preparing().unwrap();
        a0.mark_running().unwrap();
        let a1 = run.agent_mut(AgentId::new(1)).unwrap();
        a1.start_preparing().unwrap();
        a1.fail("boom").unwrap();

        run.cancel().unwrap();
        assert_eq!(run.outcome(), RunOutcome::Cancelled);
        assert_eq!(
            run.agent(AgentId::new(0)).unwrap().state(),
            &AgentState::Discarded
        );
        assert!(matches!(
            run.agent(AgentId::new(1)).unwrap().state(),
            AgentState::Failed(_)
        ));
        assert_eq!(
            run.agent(AgentId::new(2)).unwrap().state(),
            &AgentState::Discarded
        );
        // Cancelling again is a no-op, not an error.
        run.cancel().unwrap();
    }

    #[test]
    fn cannot_cancel_a_decided_run() {
        let mut run = run_with(1);
        let a = run.agent_mut(AgentId::new(0)).unwrap();
        a.start_preparing().unwrap();
        a.mark_running().unwrap();
        a.succeed(DiffStat::default()).unwrap();
        run.pick(AgentId::new(0)).unwrap();
        assert!(matches!(run.cancel(), Err(CrewError::AlreadyDecided(_))));
    }

    #[test]
    fn retry_resets_a_failed_agent_to_pending() {
        let mut run = run_with(2);
        let a0 = run.agent_mut(AgentId::new(0)).unwrap();
        a0.start_preparing().unwrap();
        a0.fail("agent crashed").unwrap();

        run.retry(
            AgentId::new(0),
            "crew/r1/0-claude-retry1",
            PathBuf::from("/wt/r1-0-retry1"),
            spec(),
        )
        .unwrap();

        let a0 = run.agent(AgentId::new(0)).unwrap();
        assert_eq!(a0.state(), &AgentState::Pending);
        assert_eq!(a0.branch(), "crew/r1/0-claude-retry1");
        assert_eq!(a0.worktree(), &PathBuf::from("/wt/r1-0-retry1"));
        assert!(a0.diff_stat().is_none());

        // It can now be driven to success like a fresh agent.
        let a0 = run.agent_mut(AgentId::new(0)).unwrap();
        a0.start_preparing().unwrap();
        a0.mark_running().unwrap();
        a0.succeed(DiffStat::default()).unwrap();
    }

    #[test]
    fn cannot_retry_a_running_or_picked_agent() {
        let mut run = run_with(1);
        let a = run.agent_mut(AgentId::new(0)).unwrap();
        a.start_preparing().unwrap();
        a.mark_running().unwrap();
        // Running → can't retry.
        assert!(matches!(
            run.retry(AgentId::new(0), "b", PathBuf::from("/wt/x"), spec()),
            Err(CrewError::IllegalTransition { .. })
        ));
        // Drive to picked, then retry is rejected because the run is decided.
        run.agent_mut(AgentId::new(0))
            .unwrap()
            .succeed(DiffStat::default())
            .unwrap();
        run.pick(AgentId::new(0)).unwrap();
        assert!(matches!(
            run.retry(AgentId::new(0), "b", PathBuf::from("/wt/x"), spec()),
            Err(CrewError::AlreadyDecided(_))
        ));
    }

    #[test]
    fn base_ref_lowers_to_git_revision() {
        assert_eq!(BaseRef::Head.as_git_ref(), "HEAD");
        assert_eq!(BaseRef::Branch("main".into()).as_git_ref(), "main");
        assert_eq!(BaseRef::Commit("abc123".into()).as_git_ref(), "abc123");
    }
}
