//! Crew controller — the binary's bridge between the headless
//! `cockpit-crew` core and the live app (v0.14 M14.5).
//!
//! Owns the run-id allocator, a snapshot of the `[crew]` config, and the
//! active [`CrewView`]. It builds a [`CrewPlan`] from config + a prompt,
//! materialises a run, and drives the *synchronous git side* through
//! `cockpit_crew::orchestrate` using the app's injected [`ProcessRunner`].
//! The long-lived agent processes themselves are spawned by the app's
//! terminal/mux layer from the [`AgentSpawn`]s this controller hands back —
//! keeping the controller headless-testable (the tests drive a
//! `FakeProcessRunner`, no real git, no PTY).

use std::path::{Path, PathBuf};

use cockpit_config::CrewConfig;
use cockpit_crew::{
    AgentId, AgentSpec, CrewPlan, CrewTask, OrchestrateError, Placeholders, RunId, orchestrate,
};
use cockpit_project::env::{ProcessRunner, ProcessSpec};
use cockpit_ui::{CrewIntent, CrewView};

/// One agent the app should spawn as a live PTY pane: its id, its isolated
/// worktree, and the fully-resolved command to run there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawn {
    pub id: AgentId,
    pub worktree: PathBuf,
    pub command: ProcessSpec,
}

/// Result of starting a run: the worktrees were prepared (git side done);
/// these agents are ready for the app to spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunStart {
    pub run_id: RunId,
    pub agents: Vec<AgentSpawn>,
}

/// Why a run couldn't be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewStartError {
    /// No agents are configured under `[crew]`.
    NoAgents,
}

/// What an applied [`CrewIntent`] resolved to, for the app to act on (open a
/// diff, attach a terminal, integrate the winner's worktree, …).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrewOutcome {
    /// No run is active; the intent was a no-op.
    NoRun,
    /// The winner was picked; its worktree is ready to integrate.
    Picked { id: AgentId, worktree: PathBuf },
    /// An agent was discarded and its worktree pruned.
    Discarded(AgentId),
    /// The run was cancelled.
    Cancelled,
    /// Re-run this agent: its worktree was re-prepared, spawn it again.
    Retried(AgentSpawn),
    /// Open this worktree's diff against the base.
    OpenDiff { id: AgentId, worktree: PathBuf },
    /// Attach a terminal to this worktree.
    OpenTerminal { id: AgentId, worktree: PathBuf },
}

/// Owns crew state for the app.
#[derive(Debug)]
pub struct CrewController {
    config: CrewConfig,
    next_run_id: u64,
    active: Option<CrewView>,
}

impl Default for CrewController {
    fn default() -> Self {
        Self::new()
    }
}

impl CrewController {
    /// New controller with default config and no active run.
    pub fn new() -> Self {
        Self {
            config: CrewConfig::default(),
            next_run_id: 1,
            active: None,
        }
    }

    /// Adopt the user's `[crew]` config (called from `apply_user_config`).
    pub fn configure(&mut self, config: &CrewConfig) {
        self.config = config.clone();
    }

    /// The active review view, if a run is in flight.
    pub fn active(&self) -> Option<&CrewView> {
        self.active.as_ref()
    }

    /// Whether a run is active.
    pub fn has_active_run(&self) -> bool {
        self.active.is_some()
    }

    /// Move focus within the active run. Positive `delta` → next.
    pub fn focus(&mut self, delta: i32) {
        if let Some(view) = self.active.as_mut() {
            if delta >= 0 {
                view.focus_next();
            } else {
                view.focus_previous();
            }
        }
    }

    /// The agent specs for a run: the configured agents, capped to
    /// `default_parallelism`.
    fn specs(&self) -> Vec<AgentSpec> {
        let cap = self.config.default_parallelism.max(1);
        self.config
            .agents
            .iter()
            .take(cap)
            .map(AgentSpec::from_config)
            .collect()
    }

    /// Start a run for `prompt`: materialise the plan and prepare every
    /// agent's worktree via git. Agents whose worktree creation fails are
    /// left `Failed` and omitted from the returned spawn list; the rest are
    /// ready for the app to spawn as PTY panes.
    pub fn start_run(
        &mut self,
        prompt: impl Into<String>,
        repo_root: &Path,
        runner: &dyn ProcessRunner,
    ) -> Result<RunStart, CrewStartError> {
        if self.config.agents.is_empty() {
            return Err(CrewStartError::NoAgents);
        }
        let run_id = RunId::new(self.next_run_id);
        self.next_run_id += 1;

        let plan = CrewPlan {
            task: CrewTask::new(prompt),
            agents: self.specs(),
            worktree_root: expand_tilde(&self.config.worktree_root),
            branch_prefix: self.config.branch_prefix.clone(),
        };
        let mut run = plan.materialise(run_id);

        let ids: Vec<AgentId> = run.agents().iter().map(|a| a.id()).collect();
        let mut agents = Vec::new();
        for id in ids {
            if orchestrate::prepare_worktree(&mut run, id, repo_root, runner).is_ok() {
                let agent = run.agent(id).expect("agent id from this run");
                agents.push(AgentSpawn {
                    id,
                    worktree: agent.worktree().clone(),
                    command: agent.command().clone(),
                });
            }
            // On failure the agent is already `Failed` in the run.
        }

        self.active = Some(CrewView::new(run));
        Ok(RunStart { run_id, agents })
    }

    /// Mark a spawned agent's process as running (the app calls this once the
    /// PTY is live). No-op if there's no active run / unknown id.
    pub fn mark_running(&mut self, id: AgentId) {
        if let Some(view) = self.active.as_mut()
            && let Some(agent) = view.run_mut().agent_mut(id)
        {
            let _ = agent.mark_running();
        }
    }

    /// Apply a review intent emitted by the [`CrewView`], driving the git
    /// side and returning what the app should do next.
    pub fn apply_intent(
        &mut self,
        intent: CrewIntent,
        repo_root: &Path,
        runner: &dyn ProcessRunner,
    ) -> Result<CrewOutcome, OrchestrateError> {
        let Some(view) = self.active.as_mut() else {
            return Ok(CrewOutcome::NoRun);
        };
        match intent {
            CrewIntent::Pick(id) => {
                orchestrate::pick(view.run_mut(), id, repo_root, runner)?;
                let worktree = view
                    .run()
                    .winner()
                    .map(|a| a.worktree().clone())
                    .unwrap_or_default();
                Ok(CrewOutcome::Picked { id, worktree })
            }
            CrewIntent::Discard(id) => {
                orchestrate::discard(view.run_mut(), id, repo_root, runner)?;
                Ok(CrewOutcome::Discarded(id))
            }
            CrewIntent::Cancel(_) => {
                orchestrate::cancel(view.run_mut(), repo_root, runner)?;
                Ok(CrewOutcome::Cancelled)
            }
            CrewIntent::OpenDiff(id) => {
                let worktree = worktree_of(view, id);
                Ok(CrewOutcome::OpenDiff { id, worktree })
            }
            CrewIntent::OpenTerminal(id) => {
                let worktree = worktree_of(view, id);
                Ok(CrewOutcome::OpenTerminal { id, worktree })
            }
            CrewIntent::Retry(id) => self.retry(id, repo_root, runner),
        }
    }

    /// Reset a failed/discarded agent into a fresh worktree and re-prepare it
    /// (M14.6). Returns an [`AgentSpawn`] for the app to re-launch.
    fn retry(
        &mut self,
        id: AgentId,
        repo_root: &Path,
        runner: &dyn ProcessRunner,
    ) -> Result<CrewOutcome, OrchestrateError> {
        let view = self
            .active
            .as_mut()
            .expect("retry called with an active run");

        // Re-resolve a fresh branch/worktree/command for the agent. The retry
        // suffix keeps the new worktree from colliding with the abandoned one.
        let (name, run_id, base, prompt) = {
            let run = view.run();
            let agent = run.agent(id).ok_or(OrchestrateError::Run(
                cockpit_crew::CrewError::UnknownAgent(id),
            ))?;
            (
                agent.agent().to_string(),
                run.id().get(),
                run.task().base.as_git_ref().to_string(),
                run.task().prompt.clone(),
            )
        };
        let attempt = self.next_run_id; // monotonic, unique-enough suffix
        self.next_run_id += 1;

        let slug = format!("{}-retry{attempt}", name);
        let branch = format!("{}/r{run_id}/{slug}", self.config.branch_prefix);
        let worktree = expand_tilde(&self.config.worktree_root).join(format!("r{run_id}-{slug}"));
        let command = self
            .config
            .agents
            .iter()
            .find(|a| a.name == name)
            .map(AgentSpec::from_config)
            .map(|spec| {
                spec.resolve(&Placeholders {
                    prompt: &prompt,
                    worktree: &worktree,
                    branch: &branch,
                    base: &base,
                })
            })
            .unwrap_or_else(|| {
                // Config no longer lists this agent: reuse the old command,
                // just retargeted at the new worktree.
                let old = view.run().agent(id).expect("agent exists").command();
                ProcessSpec {
                    current_dir: Some(worktree.clone()),
                    ..old.clone()
                }
            });

        let view = self.active.as_mut().expect("active run");
        view.run_mut()
            .retry(id, branch, worktree.clone(), command.clone())?;
        orchestrate::prepare_worktree(view.run_mut(), id, repo_root, runner)?;

        Ok(CrewOutcome::Retried(AgentSpawn {
            id,
            worktree,
            command,
        }))
    }
}

fn worktree_of(view: &CrewView, id: AgentId) -> PathBuf {
    view.run()
        .agent(id)
        .map(|a| a.worktree().clone())
        .unwrap_or_default()
}

/// Expand a leading `~/` to the user's home directory; leave other paths
/// untouched. Keeps the config schema human-friendly without baking an
/// absolute path into it.
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_config::CrewAgentConfig;
    use cockpit_crew::{AgentState, RunOutcome};
    use cockpit_project::env::{FakeProcessRunner, ProcessOutput};

    fn ok(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            success: true,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn err(stderr: &str) -> ProcessOutput {
        ProcessOutput {
            success: false,
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn controller(parallelism: usize) -> CrewController {
        let mut c = CrewController::new();
        c.configure(&CrewConfig {
            worktree_root: "/cache/crew".into(),
            branch_prefix: "crew".into(),
            default_parallelism: parallelism,
            agents: vec![
                CrewAgentConfig {
                    name: "claude".into(),
                    program: "claude".into(),
                    args: vec!["-p".into(), "{prompt}".into()],
                },
                CrewAgentConfig {
                    name: "codex".into(),
                    program: "codex".into(),
                    args: vec!["exec".into(), "{prompt}".into()],
                },
            ],
        });
        c
    }

    #[test]
    fn start_run_prepares_worktrees_and_caps_parallelism() {
        let mut c = controller(1); // only the first agent participates
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            [
                "worktree",
                "add",
                "-b",
                "crew/r1/0-claude",
                "/cache/crew/r1-0-claude",
                "HEAD",
            ],
            ok(""),
        );
        let start = c.start_run("fix it", Path::new("/repo"), &runner).unwrap();
        assert_eq!(start.agents.len(), 1);
        assert_eq!(
            start.agents[0].worktree,
            PathBuf::from("/cache/crew/r1-0-claude")
        );
        assert!(c.has_active_run());
    }

    #[test]
    fn no_agents_configured_is_an_error() {
        let mut c = CrewController::new();
        c.configure(&CrewConfig {
            agents: vec![],
            ..CrewConfig::default()
        });
        let runner = FakeProcessRunner::new();
        assert_eq!(
            c.start_run("x", Path::new("/repo"), &runner),
            Err(CrewStartError::NoAgents)
        );
    }

    #[test]
    fn pick_returns_winner_worktree_and_prunes_loser() {
        let mut c = controller(2);
        let runner = FakeProcessRunner::new();
        for (i, name) in [(0, "claude"), (1, "codex")] {
            runner.expect(
                "git",
                [
                    "worktree".to_string(),
                    "add".into(),
                    "-b".into(),
                    format!("crew/r1/{i}-{name}"),
                    format!("/cache/crew/r1-{i}-{name}"),
                    "HEAD".into(),
                ],
                ok(""),
            );
            runner.expect("git", ["diff", "--numstat", "HEAD"], ok("1\t0\ta.rs\n"));
        }
        let start = c.start_run("go", Path::new("/repo"), &runner).unwrap();

        // Mark running + finalize both via the orchestrator helpers.
        for spawn in &start.agents {
            c.mark_running(spawn.id);
            orchestrate::finalize_success(c.active.as_mut().unwrap().run_mut(), spawn.id, &runner)
                .unwrap();
        }

        let winner = start.agents[0].id;
        let loser_wt = start.agents[1].worktree.to_string_lossy().into_owned();
        runner.expect("git", ["worktree", "remove", "--force", &loser_wt], ok(""));

        let outcome = c
            .apply_intent(CrewIntent::Pick(winner), Path::new("/repo"), &runner)
            .unwrap();
        match outcome {
            CrewOutcome::Picked { id, worktree } => {
                assert_eq!(id, winner);
                assert_eq!(worktree, start.agents[0].worktree);
            }
            other => panic!("expected Picked, got {other:?}"),
        }
        assert_eq!(
            c.active().unwrap().run().outcome(),
            RunOutcome::Decided(winner)
        );
    }

    #[test]
    fn apply_intent_without_a_run_is_a_noop() {
        let mut c = controller(2);
        let runner = FakeProcessRunner::new();
        let outcome = c
            .apply_intent(
                CrewIntent::Cancel(RunId::new(1)),
                Path::new("/repo"),
                &runner,
            )
            .unwrap();
        assert_eq!(outcome, CrewOutcome::NoRun);
    }

    #[test]
    fn retry_reprepares_a_failed_agent() {
        let mut c = controller(1);
        let runner = FakeProcessRunner::new();
        // First prepare fails → agent Failed.
        runner.expect(
            "git",
            [
                "worktree",
                "add",
                "-b",
                "crew/r1/0-claude",
                "/cache/crew/r1-0-claude",
                "HEAD",
            ],
            err("fatal: boom"),
        );
        let start = c.start_run("go", Path::new("/repo"), &runner).unwrap();
        assert_eq!(start.agents.len(), 0); // none spawned
        let id = c.active().unwrap().run().agents()[0].id();
        assert!(matches!(
            c.active().unwrap().run().agents()[0].state(),
            AgentState::Failed(_)
        ));

        // Retry: fresh worktree with a retry suffix is prepared.
        runner.expect(
            "git",
            [
                "worktree",
                "add",
                "-b",
                "crew/r1/claude-retry2",
                "/cache/crew/r1-claude-retry2",
                "HEAD",
            ],
            ok(""),
        );
        let outcome = c
            .apply_intent(CrewIntent::Retry(id), Path::new("/repo"), &runner)
            .unwrap();
        match outcome {
            CrewOutcome::Retried(spawn) => {
                assert_eq!(spawn.id, id);
                assert_eq!(
                    spawn.worktree,
                    PathBuf::from("/cache/crew/r1-claude-retry2")
                );
            }
            other => panic!("expected Retried, got {other:?}"),
        }
        assert_eq!(
            c.active().unwrap().run().agents()[0].state(),
            &AgentState::Preparing
        );
    }
}
