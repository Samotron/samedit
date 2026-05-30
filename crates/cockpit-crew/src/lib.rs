//! `cockpit-crew` — headless core for v0.14 **agent crews** (M14.1–M14.3).
//!
//! Inspired by [cmux]: run several coding agents in parallel on the *same*
//! task, each in its own isolated git worktree, then review their diffs
//! side-by-side and pick a winner to integrate. Where cmux orchestrates
//! containers, the cockpit reuses what it already has — `git worktree` for
//! isolation, the `ProcessRunner` seam for spawning, and the command spine
//! for dispatch — so a crew is "just" another view-model over the existing
//! project/terminal machinery.
//!
//! This crate is the backend-free brain, split three ways:
//!
//! - [`run`] — the [`CrewRun`] aggregate and the per-agent state machine
//!   (`Pending → Preparing → Running → Succeeded | Failed`, then `Picked` /
//!   `Discarded`). Every transition is guarded and returns a [`CrewError`].
//! - [`worktree`] — git worktree command construction + porcelain parsing,
//!   with the spawn injected through [`cockpit_project::env::ProcessRunner`]
//!   (the same pattern as `cockpit_project::git`).
//! - [`spec`] — [`AgentSpec`] / [`CrewPlan`]: the bridge from
//!   `cockpit-config`'s `[crew]` section to a live [`CrewRun`], including
//!   `{prompt}`/`{worktree}`/`{branch}`/`{base}` placeholder expansion.
//!
//! Stable command ids live in [`command_ids`]; the UI view-model, the
//! `cockpit-quick`-style binary wiring, and the actual PTY spawn all live
//! outside this crate. Headless and unit-tested per the AGENTS.md hard rules:
//! no window, no GPU, no PTY, no real filesystem (tests use the
//! `FakeProcessRunner`), no network.
//!
//! [cmux]: https://github.com/manaflow-ai/cmux

pub mod command_ids;
pub mod run;
pub mod spec;
pub mod worktree;

pub use run::{
    AgentId, AgentRun, AgentState, BaseRef, CrewError, CrewRun, CrewTask, DiffStat, RunId,
    RunOutcome,
};
pub use spec::{AgentSpec, CrewPlan, Placeholders};
pub use worktree::{WorktreeEntry, WorktreeError};

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::env::{FakeProcessRunner, ProcessOutput};
    use std::path::{Path, PathBuf};

    fn ok(stdout: &str) -> ProcessOutput {
        ProcessOutput {
            success: true,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// End-to-end: a plan fans out to two agents, the orchestration drives
    /// each through its worktree lifecycle using a fake git, both succeed,
    /// and picking one discards the other. Exercises run + spec + worktree
    /// together without touching real git or a real PTY.
    #[test]
    fn plan_to_pick_with_fake_git() {
        let repo = Path::new("/repo");
        let plan = CrewPlan {
            task: CrewTask::new("speed up startup"),
            agents: vec![
                AgentSpec::new("claude", "claude", vec!["-p".into(), "{prompt}".into()]),
                AgentSpec::new("codex", "codex", vec!["exec".into(), "{prompt}".into()]),
            ],
            worktree_root: PathBuf::from("/cache/crew"),
            branch_prefix: "crew".into(),
        };
        let mut run = plan.materialise(RunId::new(1));

        // Script the git worktree create + diff for both agents.
        let runner = FakeProcessRunner::new();
        for (idx, name) in [(0u64, "claude"), (1, "codex")] {
            let branch = format!("crew/r1/{idx}-{name}");
            let wt = format!("/cache/crew/r1-{idx}-{name}");
            runner.expect(
                "git",
                ["worktree", "add", "-b", &branch, &wt, "HEAD"],
                ok(""),
            );
            runner.expect(
                "git",
                ["diff", "--numstat", "HEAD"],
                ok("1\t1\tsrc/main.rs\n"),
            );
        }

        // Drive each agent: create worktree → run → capture diff → succeed.
        let ids: Vec<AgentId> = run.agents().iter().map(|a| a.id()).collect();
        for id in ids {
            let (branch, worktree) = {
                let a = run.agent(id).unwrap();
                (a.branch().to_string(), a.worktree().clone())
            };
            run.agent_mut(id).unwrap().start_preparing().unwrap();
            worktree::create_worktree(&runner, repo, &worktree, &branch, "HEAD").unwrap();
            run.agent_mut(id).unwrap().mark_running().unwrap();
            let stat = worktree::diff_stat(&runner, &worktree, "HEAD").unwrap();
            run.agent_mut(id).unwrap().succeed(stat).unwrap();
        }

        assert!(run.all_settled());
        assert_eq!(run.reviewable_count(), 2);

        // Pick the first; the second is discarded.
        let winner = run.agents()[0].id();
        run.pick(winner).unwrap();
        assert_eq!(run.winner().unwrap().id(), winner);
        assert_eq!(run.agents()[1].state(), &AgentState::Discarded);
        assert_eq!(run.winner().unwrap().diff_stat().unwrap().files_changed, 1);
    }
}
