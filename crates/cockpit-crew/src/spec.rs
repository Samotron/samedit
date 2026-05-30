//! Agent specs and run plans — the bridge from config to a live [`CrewRun`].
//!
//! An [`AgentSpec`] is the recipe for one agent: a name and a command
//! template with `{prompt}`, `{worktree}`, `{branch}`, and `{base}`
//! placeholders. A [`CrewPlan`] bundles the task with the set of agents and
//! the worktree layout, then [`CrewPlan::materialise`] stamps out a
//! [`CrewRun`] with each agent's branch, worktree path, and fully-resolved
//! command. Pure data + string substitution — no git, no spawning.

use std::path::{Path, PathBuf};

use cockpit_config::CrewAgentConfig;
use cockpit_project::env::ProcessSpec;

use crate::run::{AgentId, AgentRun, CrewRun, CrewTask, RunId};

/// The recipe for one agent: how to invoke it. The args carry placeholders
/// expanded per-run by [`AgentSpec::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpec {
    /// Display name and worktree/branch slug (e.g. `claude`).
    pub name: String,
    /// Program to spawn (resolved against `$PATH`).
    pub program: String,
    /// Argument template; placeholders are expanded per run.
    pub args: Vec<String>,
}

impl AgentSpec {
    /// Build a spec directly.
    pub fn new(name: impl Into<String>, program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args,
        }
    }

    /// Adapt a `[[crew.agents]]` config entry into a spec.
    pub fn from_config(config: &CrewAgentConfig) -> Self {
        Self {
            name: config.name.clone(),
            program: config.program.clone(),
            args: config.args.clone(),
        }
    }

    /// Resolve the command template against a placeholder context, producing
    /// a spawnable [`ProcessSpec`] whose working directory is the agent's
    /// worktree.
    pub fn resolve(&self, ctx: &Placeholders<'_>) -> ProcessSpec {
        let args = self.args.iter().map(|arg| ctx.expand(arg));
        ProcessSpec::new(ctx.expand(&self.program))
            .args(args)
            .current_dir(ctx.worktree)
    }
}

/// The values substituted into an [`AgentSpec`]'s template for one agent.
#[derive(Debug, Clone, Copy)]
pub struct Placeholders<'a> {
    /// The task prompt.
    pub prompt: &'a str,
    /// The agent's worktree path.
    pub worktree: &'a Path,
    /// The agent's branch name.
    pub branch: &'a str,
    /// The base revision the run branched off.
    pub base: &'a str,
}

impl Placeholders<'_> {
    fn expand(&self, template: &str) -> String {
        template
            .replace("{prompt}", self.prompt)
            .replace("{worktree}", &self.worktree.to_string_lossy())
            .replace("{branch}", self.branch)
            .replace("{base}", self.base)
    }
}

/// A plan for a run: the task, the participating agents, and where the
/// per-agent worktrees and branches live. [`CrewPlan::materialise`] turns it
/// into a [`CrewRun`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewPlan {
    /// The shared task.
    pub task: CrewTask,
    /// One entry per agent to fan out to.
    pub agents: Vec<AgentSpec>,
    /// Directory under which each agent's worktree is created.
    pub worktree_root: PathBuf,
    /// Prefix for the per-agent branch names.
    pub branch_prefix: String,
}

impl CrewPlan {
    /// Stamp out a [`CrewRun`] for `run_id`. Each agent gets a unique branch
    /// `<prefix>/r<run>/<index>-<name>` and a worktree
    /// `<worktree_root>/r<run>-<index>-<name>`, with its command resolved
    /// against those values. The index keeps names unique even when two
    /// agents share a `name`.
    pub fn materialise(&self, run_id: RunId) -> CrewRun {
        let run = run_id.get();
        let base = self.task.base.as_git_ref();
        let agents = self
            .agents
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let slug = format!("{index}-{}", spec.name);
                let branch = format!("{}/r{run}/{slug}", self.branch_prefix);
                let worktree = self.worktree_root.join(format!("r{run}-{slug}"));
                let command = spec.resolve(&Placeholders {
                    prompt: &self.task.prompt,
                    worktree: &worktree,
                    branch: &branch,
                    base,
                });
                AgentRun::new(
                    AgentId::new(index as u64),
                    &spec.name,
                    branch,
                    worktree,
                    command,
                )
            })
            .collect();
        CrewRun::new(run_id, self.task.clone(), agents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::BaseRef;

    #[test]
    fn resolve_expands_every_placeholder() {
        let spec = AgentSpec::new(
            "claude",
            "claude",
            vec![
                "-p".into(),
                "{prompt}".into(),
                "--cwd".into(),
                "{worktree}".into(),
            ],
        );
        let wt = PathBuf::from("/wt/r1-0-claude");
        let command = spec.resolve(&Placeholders {
            prompt: "fix the bug",
            worktree: &wt,
            branch: "crew/r1/0-claude",
            base: "HEAD",
        });
        assert_eq!(command.program, "claude");
        let args: Vec<_> = command
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["-p", "fix the bug", "--cwd", "/wt/r1-0-claude"]);
        assert_eq!(command.current_dir.as_deref(), Some(wt.as_path()));
    }

    #[test]
    fn materialise_builds_unique_branches_and_worktrees() {
        let plan = CrewPlan {
            task: CrewTask::new("add a feature").base(BaseRef::Branch("main".into())),
            agents: vec![
                AgentSpec::new("claude", "claude", vec!["-p".into(), "{prompt}".into()]),
                // Two agents sharing a name still get distinct slots.
                AgentSpec::new("claude", "claude", vec!["-p".into(), "{prompt}".into()]),
                AgentSpec::new("codex", "codex", vec!["exec".into(), "{prompt}".into()]),
            ],
            worktree_root: PathBuf::from("/cache/crew"),
            branch_prefix: "crew".into(),
        };
        let run = plan.materialise(RunId::new(7));

        assert_eq!(run.agents().len(), 3);
        let branches: Vec<&str> = run.agents().iter().map(|a| a.branch()).collect();
        assert_eq!(
            branches,
            ["crew/r7/0-claude", "crew/r7/1-claude", "crew/r7/2-codex"]
        );
        assert_eq!(
            run.agents()[0].worktree(),
            &PathBuf::from("/cache/crew/r7-0-claude")
        );
        assert_eq!(
            run.agents()[2].worktree(),
            &PathBuf::from("/cache/crew/r7-2-codex")
        );

        // The base revision flows into the prompt-free args untouched, and
        // the prompt is expanded.
        let args: Vec<_> = run.agents()[2]
            .command()
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["exec", "add a feature"]);
    }

    #[test]
    fn from_config_round_trips_fields() {
        let cfg = CrewAgentConfig {
            name: "codex".into(),
            program: "codex".into(),
            args: vec!["exec".into(), "{prompt}".into()],
        };
        let spec = AgentSpec::from_config(&cfg);
        assert_eq!(spec.name, "codex");
        assert_eq!(spec.program, "codex");
        assert_eq!(spec.args, ["exec", "{prompt}"]);
    }
}
