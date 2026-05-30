//! `[crew]` configuration for v0.14 agent crews (M14.2).
//!
//! Lives in the main `config.toml` (a crew is an in-cockpit feature, not a
//! sibling binary like the launcher), surfaced as [`Config::crew`]. Pure
//! data; `cockpit-crew` maps [`CrewAgentConfig`] onto its `AgentSpec` and
//! never depends on this crate the other way round.
//!
//! ```toml
//! [crew]
//! worktree_root = "~/.cache/cockpit/crew"
//! branch_prefix = "crew"
//! default_parallelism = 3
//!
//! [[crew.agents]]
//! name    = "claude"
//! program = "claude"
//! args    = ["-p", "{prompt}"]
//!
//! [[crew.agents]]
//! name    = "codex"
//! program = "codex"
//! args    = ["exec", "{prompt}"]
//! ```

use serde::{Deserialize, Serialize};

/// `[crew]` — parallel agent runs over isolated git worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CrewConfig {
    /// Directory under which per-agent worktrees are created. `~` is the
    /// caller's to expand; the schema keeps it verbatim.
    pub worktree_root: String,
    /// Prefix for the per-agent branch names (`<prefix>/r<run>/<slug>`).
    pub branch_prefix: String,
    /// How many agents a fresh run fans out to by default.
    pub default_parallelism: usize,
    /// The agents available to a crew run. The first `default_parallelism`
    /// are used unless the user picks explicitly.
    pub agents: Vec<CrewAgentConfig>,
}

impl Default for CrewConfig {
    fn default() -> Self {
        Self {
            worktree_root: "~/.cache/cockpit/crew".to_string(),
            branch_prefix: "crew".to_string(),
            default_parallelism: 3,
            agents: vec![
                CrewAgentConfig {
                    name: "claude".to_string(),
                    program: "claude".to_string(),
                    args: vec!["-p".to_string(), "{prompt}".to_string()],
                },
                CrewAgentConfig {
                    name: "codex".to_string(),
                    program: "codex".to_string(),
                    args: vec!["exec".to_string(), "{prompt}".to_string()],
                },
            ],
        }
    }
}

/// One `[[crew.agents]]` entry: an agent recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrewAgentConfig {
    /// Display name and worktree/branch slug (e.g. `claude`).
    pub name: String,
    /// Program to spawn.
    pub program: String,
    /// Argument template; `cockpit-crew` expands `{prompt}` / `{worktree}` /
    /// `{branch}` / `{base}` placeholders per run.
    #[serde(default)]
    pub args: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    #[test]
    fn default_ships_two_agents() {
        let crew = CrewConfig::default();
        assert_eq!(crew.default_parallelism, 3);
        let names: Vec<&str> = crew.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["claude", "codex"]);
    }

    #[test]
    fn parses_crew_section_from_config() {
        let config = Config::from_toml(
            r#"
[crew]
worktree_root = "/tmp/crew"
default_parallelism = 2

[[crew.agents]]
name = "claude"
program = "claude"
args = ["-p", "{prompt}"]

[[crew.agents]]
name = "aider"
program = "aider"
args = ["--message", "{prompt}"]
"#,
        )
        .unwrap();

        assert_eq!(config.crew.worktree_root, "/tmp/crew");
        assert_eq!(config.crew.default_parallelism, 2);
        // branch_prefix falls back to its default when omitted.
        assert_eq!(config.crew.branch_prefix, "crew");
        let names: Vec<&str> = config.crew.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["claude", "aider"]);
        assert_eq!(config.crew.agents[1].args, ["--message", "{prompt}"]);
    }

    #[test]
    fn unknown_agent_key_is_rejected() {
        let err = Config::from_toml(
            r#"
[[crew.agents]]
name = "x"
program = "x"
model = "oops"
"#,
        );
        assert!(err.is_err(), "deny_unknown_fields should reject `model`");
    }
}
