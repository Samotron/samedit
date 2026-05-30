//! Crew orchestration — the synchronous git side of driving a run.
//!
//! These helpers sit between the pure [`CrewRun`] state machine and the
//! binary's PTY/event loop. The binary calls them at the *edges* of an
//! agent's life: [`prepare_worktree`] before it spawns the agent process,
//! [`finalize_success`] once the PTY exits cleanly, and
//! [`discard`]/[`pick`]/[`cancel`] when the user reviews. The long-lived
//! agent process itself is the binary's job (a `cockpit-mux` pane over a real
//! PTY) — this module never spawns it, so it stays headless-testable: the
//! tests drive a `FakeProcessRunner` and the state machine, no real git.
//!
//! Each helper keeps the run model and the on-disk worktrees in step: a
//! transition that the state machine rejects short-circuits before any git
//! runs, and a git failure during `prepare` rolls the agent into `Failed` so
//! the run never claims an agent is live when its worktree never appeared.

use std::collections::BTreeSet;
use std::path::Path;

use cockpit_project::env::ProcessRunner;
use thiserror::Error;

use crate::run::{AgentId, AgentState, CrewError, CrewRun, DiffStat};
use crate::worktree::{self, WorktreeError};

/// Failure from an orchestration step — either the state machine refused the
/// transition or git itself failed.
#[derive(Debug, Error)]
pub enum OrchestrateError {
    /// The [`CrewRun`] state machine rejected the transition.
    #[error(transparent)]
    Run(#[from] CrewError),
    /// A git worktree command failed.
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
}

/// Create the agent's isolated worktree and move it `Pending → Preparing`.
///
/// On a git failure the agent is rolled into `Failed` (the worktree never
/// materialised, so the run must reflect that) and the error is returned.
pub fn prepare_worktree(
    run: &mut CrewRun,
    id: AgentId,
    repo_root: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), OrchestrateError> {
    let (branch, worktree, base) = {
        let agent = run.agent(id).ok_or(CrewError::UnknownAgent(id))?;
        (
            agent.branch().to_string(),
            agent.worktree().clone(),
            run.task().base.as_git_ref().to_string(),
        )
    };
    run.agent_mut(id)
        .ok_or(CrewError::UnknownAgent(id))?
        .start_preparing()?;

    match worktree::create_worktree(runner, repo_root, &worktree, &branch, &base) {
        Ok(()) => Ok(()),
        Err(err) => {
            // Keep the model honest: no worktree ⇒ the agent failed.
            if let Some(agent) = run.agent_mut(id) {
                let _ = agent.fail(format!("worktree: {err}"));
            }
            Err(err.into())
        }
    }
}

/// Capture the agent worktree's diff against the base and move it
/// `Running → Succeeded(diff)`. Returns the captured [`DiffStat`].
pub fn finalize_success(
    run: &mut CrewRun,
    id: AgentId,
    runner: &dyn ProcessRunner,
) -> Result<DiffStat, OrchestrateError> {
    let (worktree, base) = {
        let agent = run.agent(id).ok_or(CrewError::UnknownAgent(id))?;
        (
            agent.worktree().clone(),
            run.task().base.as_git_ref().to_string(),
        )
    };
    let stat = worktree::diff_stat(runner, &worktree, &base)?;
    run.agent_mut(id)
        .ok_or(CrewError::UnknownAgent(id))?
        .succeed(stat)?;
    Ok(stat)
}

/// Discard a single agent and prune its worktree.
pub fn discard(
    run: &mut CrewRun,
    id: AgentId,
    repo_root: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), OrchestrateError> {
    let worktree = run
        .agent(id)
        .ok_or(CrewError::UnknownAgent(id))?
        .worktree()
        .clone();
    run.discard(id)?;
    worktree::remove_worktree(runner, repo_root, &worktree)?;
    Ok(())
}

/// Pick the winner, then prune every worktree this step discarded — leaving
/// the winner's worktree in place for the binary to integrate (merge / open).
/// Worktrees discarded by an *earlier* [`discard`] call are left alone (they
/// were already pruned then).
pub fn pick(
    run: &mut CrewRun,
    id: AgentId,
    repo_root: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), OrchestrateError> {
    let already_discarded = discarded_ids(run);
    run.pick(id)?;
    prune_newly_discarded(run, &already_discarded, repo_root, runner)
}

/// Cancel the run and prune every worktree this step discarded.
pub fn cancel(
    run: &mut CrewRun,
    repo_root: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), OrchestrateError> {
    let already_discarded = discarded_ids(run);
    run.cancel()?;
    prune_newly_discarded(run, &already_discarded, repo_root, runner)
}

fn discarded_ids(run: &CrewRun) -> BTreeSet<AgentId> {
    run.agents()
        .iter()
        .filter(|a| matches!(a.state(), AgentState::Discarded))
        .map(|a| a.id())
        .collect()
}

fn prune_newly_discarded(
    run: &CrewRun,
    already_discarded: &BTreeSet<AgentId>,
    repo_root: &Path,
    runner: &dyn ProcessRunner,
) -> Result<(), OrchestrateError> {
    let to_prune: Vec<_> = run
        .agents()
        .iter()
        .filter(|a| {
            matches!(a.state(), AgentState::Discarded) && !already_discarded.contains(&a.id())
        })
        .map(|a| a.worktree().clone())
        .collect();
    for worktree in to_prune {
        worktree::remove_worktree(runner, repo_root, &worktree)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{CrewTask, RunId};
    use crate::spec::{AgentSpec, CrewPlan};
    use cockpit_project::env::{FakeProcessRunner, ProcessOutput};
    use std::path::PathBuf;

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

    fn plan(n: usize) -> CrewPlan {
        let agents = (0..n)
            .map(|i| {
                AgentSpec::new(
                    format!("agent{i}"),
                    "claude",
                    vec!["-p".into(), "{prompt}".into()],
                )
            })
            .collect();
        CrewPlan {
            task: CrewTask::new("do it"),
            agents,
            worktree_root: PathBuf::from("/cache/crew"),
            branch_prefix: "crew".into(),
        }
    }

    fn add_args(branch: &str, wt: &str) -> [String; 6] {
        [
            "worktree".into(),
            "add".into(),
            "-b".into(),
            branch.into(),
            wt.into(),
            "HEAD".into(),
        ]
    }

    #[test]
    fn prepare_creates_worktree_and_moves_to_preparing() {
        let mut run = plan(1).materialise(RunId::new(1));
        let id = run.agents()[0].id();
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            add_args("crew/r1/0-agent0", "/cache/crew/r1-0-agent0"),
            ok(""),
        );
        prepare_worktree(&mut run, id, Path::new("/repo"), &runner).unwrap();
        assert_eq!(run.agent(id).unwrap().state(), &AgentState::Preparing);
    }

    #[test]
    fn prepare_failure_marks_agent_failed() {
        let mut run = plan(1).materialise(RunId::new(1));
        let id = run.agents()[0].id();
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            add_args("crew/r1/0-agent0", "/cache/crew/r1-0-agent0"),
            err("fatal: already exists"),
        );
        let result = prepare_worktree(&mut run, id, Path::new("/repo"), &runner);
        assert!(matches!(result, Err(OrchestrateError::Worktree(_))));
        assert!(matches!(
            run.agent(id).unwrap().state(),
            AgentState::Failed(_)
        ));
    }

    #[test]
    fn full_flow_prepare_finalize_pick_prunes_only_the_loser() {
        let mut run = plan(2).materialise(RunId::new(1));
        let ids: Vec<AgentId> = run.agents().iter().map(|a| a.id()).collect();
        let runner = FakeProcessRunner::new();

        for (i, &id) in ids.iter().enumerate() {
            let branch = format!("crew/r1/{i}-agent{i}");
            let wt = format!("/cache/crew/r1-{i}-agent{i}");
            runner.expect("git", add_args(&branch, &wt), ok(""));
            runner.expect("git", ["diff", "--numstat", "HEAD"], ok("2\t1\tsrc/a.rs\n"));

            prepare_worktree(&mut run, id, Path::new("/repo"), &runner).unwrap();
            run.agent_mut(id).unwrap().mark_running().unwrap();
            let stat = finalize_success(&mut run, id, &runner).unwrap();
            assert_eq!(stat.files_changed, 1);
        }

        // Pick agent 0; only agent 1's worktree (the loser) is pruned.
        runner.expect(
            "git",
            ["worktree", "remove", "--force", "/cache/crew/r1-1-agent1"],
            ok(""),
        );
        pick(&mut run, ids[0], Path::new("/repo"), &runner).unwrap();

        assert_eq!(run.agent(ids[0]).unwrap().state(), &AgentState::Picked);
        assert_eq!(run.agent(ids[1]).unwrap().state(), &AgentState::Discarded);
        // Exactly one remove was issued (the winner's worktree is kept).
        let removes = runner
            .spawns()
            .iter()
            .filter(|s| s.args.iter().any(|a| a == "remove"))
            .count();
        assert_eq!(removes, 1);
    }

    #[test]
    fn cancel_prunes_live_worktrees() {
        let mut run = plan(2).materialise(RunId::new(1));
        let ids: Vec<AgentId> = run.agents().iter().map(|a| a.id()).collect();
        let runner = FakeProcessRunner::new();

        // Prepare both (worktrees now exist on disk).
        for (i, &id) in ids.iter().enumerate() {
            let branch = format!("crew/r1/{i}-agent{i}");
            let wt = format!("/cache/crew/r1-{i}-agent{i}");
            runner.expect("git", add_args(&branch, &wt), ok(""));
            prepare_worktree(&mut run, id, Path::new("/repo"), &runner).unwrap();
        }

        runner.expect(
            "git",
            ["worktree", "remove", "--force", "/cache/crew/r1-0-agent0"],
            ok(""),
        );
        runner.expect(
            "git",
            ["worktree", "remove", "--force", "/cache/crew/r1-1-agent1"],
            ok(""),
        );
        cancel(&mut run, Path::new("/repo"), &runner).unwrap();

        assert_eq!(run.outcome(), crate::run::RunOutcome::Cancelled);
        let removes = runner
            .spawns()
            .iter()
            .filter(|s| s.args.iter().any(|a| a == "remove"))
            .count();
        assert_eq!(removes, 2);
    }
}
