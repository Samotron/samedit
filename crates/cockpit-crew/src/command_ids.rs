//! Stable command ids for crew operations (v0.14 M14.2).
//!
//! These dot-separated ids dispatch through `cockpit-commands` like every
//! other command (AGENTS §2 #5: commands are the single spine). The binary
//! registers handlers against them; keybindings, the palette, and tests all
//! drive the same ids. Defined here, next to the model they act on, mirroring
//! `cockpit-mux`'s `command_ids` module.

/// Start a new crew run from the current prompt (opens the prompt UI).
pub const RUN_NEW: &str = "crew.run.new";
/// Cancel the active run: kill agents, prune their worktrees.
pub const RUN_CANCEL: &str = "crew.run.cancel";
/// Switch focus to the next run.
pub const RUN_NEXT: &str = "crew.run.next";
/// Switch focus to the previous run.
pub const RUN_PREVIOUS: &str = "crew.run.previous";

/// Pick the focused agent as the winner; integrate it, discard the rest.
pub const AGENT_PICK: &str = "crew.agent.pick";
/// Discard the focused agent and prune its worktree.
pub const AGENT_DISCARD: &str = "crew.agent.discard";
/// Re-run the focused agent in a fresh worktree.
pub const AGENT_RETRY: &str = "crew.agent.retry";
/// Open the focused agent's diff against the base in the editor.
pub const AGENT_OPEN_DIFF: &str = "crew.agent.open_diff";
/// Attach a terminal pane to the focused agent's worktree.
pub const AGENT_OPEN_TERMINAL: &str = "crew.agent.open_terminal";
/// Focus the next agent within the run.
pub const AGENT_FOCUS_NEXT: &str = "crew.agent.focus_next";
/// Focus the previous agent within the run.
pub const AGENT_FOCUS_PREVIOUS: &str = "crew.agent.focus_previous";

/// Every crew command id, for registry wiring and uniqueness tests.
pub const ALL: [&str; 11] = [
    RUN_NEW,
    RUN_CANCEL,
    RUN_NEXT,
    RUN_PREVIOUS,
    AGENT_PICK,
    AGENT_DISCARD,
    AGENT_RETRY,
    AGENT_OPEN_DIFF,
    AGENT_OPEN_TERMINAL,
    AGENT_FOCUS_NEXT,
    AGENT_FOCUS_PREVIOUS,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ids_are_unique_and_namespaced() {
        let set: BTreeSet<&str> = ALL.iter().copied().collect();
        assert_eq!(set.len(), ALL.len(), "duplicate command id");
        assert!(ALL.iter().all(|id| id.starts_with("crew.")));
    }
}
