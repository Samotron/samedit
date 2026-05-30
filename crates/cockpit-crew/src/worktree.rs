//! Git worktree orchestration — the only place v0.14 touches git.
//!
//! Each agent in a crew run works in its own `git worktree`, so the agents
//! never trample each other's files and the user can diff each candidate
//! against the base independently. This module is the cmux-style glue:
//! pure command *construction* plus output *parsing*, with the actual spawn
//! injected through [`ProcessRunner`] exactly like
//! [`cockpit_project::git`]. That keeps it headless-testable — the tests
//! drive a `FakeProcessRunner` and never shell out to real git.

use std::path::Path;

use cockpit_project::env::{ProcessRunner, ProcessSpec};
use thiserror::Error;

use crate::run::DiffStat;

/// One entry from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute path of the worktree.
    pub path: String,
    /// Checked-out commit, if reported.
    pub head: Option<String>,
    /// Short branch name (`refs/heads/` stripped), if not detached.
    pub branch: Option<String>,
}

/// `git worktree add -b <branch> <path> <base>` in `repo_root`.
pub fn add_worktree_spec(repo_root: &Path, path: &Path, branch: &str, base: &str) -> ProcessSpec {
    ProcessSpec::new("git")
        .args([
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path.as_os_str(),
            base.as_ref(),
        ])
        .current_dir(repo_root)
}

/// `git worktree remove --force <path>` in `repo_root`. `--force` because an
/// abandoned agent worktree usually has uncommitted changes we mean to drop.
pub fn remove_worktree_spec(repo_root: &Path, path: &Path) -> ProcessSpec {
    ProcessSpec::new("git")
        .args([
            "worktree".as_ref(),
            "remove".as_ref(),
            "--force".as_ref(),
            path.as_os_str(),
        ])
        .current_dir(repo_root)
}

/// `git worktree list --porcelain` in `repo_root`.
pub fn list_worktrees_spec(repo_root: &Path) -> ProcessSpec {
    ProcessSpec::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
}

/// `git diff --numstat <base>` run *inside* `worktree` — the agent's
/// working-tree changes relative to the run's base revision.
pub fn diff_numstat_spec(worktree: &Path, base: &str) -> ProcessSpec {
    ProcessSpec::new("git")
        .args(["diff", "--numstat", base])
        .current_dir(worktree)
}

/// Parse `git worktree list --porcelain`. Blocks are blank-line separated;
/// each begins with a `worktree <path>` line, optionally followed by
/// `HEAD <sha>` and `branch refs/heads/<name>` (or `detached`).
pub fn parse_worktree_list(stdout: &[u8]) -> Vec<WorktreeEntry> {
    let text = String::from_utf8_lossy(stdout);
    let mut out = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(entry) = current.take() {
                out.push(entry);
            }
            current = Some(WorktreeEntry {
                path: path.to_string(),
                head: None,
                branch: None,
            });
        } else if let Some(entry) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                entry.head = Some(head.to_string());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                entry.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch)
                        .to_string(),
                );
            }
        }
    }
    if let Some(entry) = current.take() {
        out.push(entry);
    }
    out
}

/// Parse `git diff --numstat` into a [`DiffStat`]. Each line is
/// `<added>\t<deleted>\t<path>`; binary files report `-\t-` and contribute a
/// changed file but no line counts.
pub fn parse_numstat(stdout: &[u8]) -> DiffStat {
    let text = String::from_utf8_lossy(stdout);
    let mut stat = DiffStat::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let added = cols.next().unwrap_or("");
        let deleted = cols.next().unwrap_or("");
        let path = cols.next().unwrap_or("");
        if path.is_empty() {
            continue;
        }
        stat.files_changed += 1;
        stat.insertions += added.parse::<u32>().unwrap_or(0);
        stat.deletions += deleted.parse::<u32>().unwrap_or(0);
    }
    stat
}

/// Errors from running a git worktree command.
#[derive(Debug, Error)]
pub enum WorktreeError {
    /// The spawn itself failed (git missing, permission, …).
    #[error("failed to spawn git: {0}")]
    Spawn(#[from] std::io::Error),
    /// git ran but exited non-zero.
    #[error("git {command} failed: {stderr}")]
    Git {
        /// The git subcommand that failed (e.g. `worktree add`).
        command: String,
        /// Captured stderr, trimmed.
        stderr: String,
    },
}

/// Create the agent's worktree: `git worktree add -b <branch> <path> <base>`.
pub fn create_worktree(
    runner: &dyn ProcessRunner,
    repo_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<(), WorktreeError> {
    run_ok(
        runner,
        &add_worktree_spec(repo_root, path, branch, base),
        "worktree add",
    )
}

/// Prune the agent's worktree: `git worktree remove --force <path>`.
pub fn remove_worktree(
    runner: &dyn ProcessRunner,
    repo_root: &Path,
    path: &Path,
) -> Result<(), WorktreeError> {
    run_ok(
        runner,
        &remove_worktree_spec(repo_root, path),
        "worktree remove",
    )
}

/// List worktrees registered on `repo_root`.
pub fn list_worktrees(
    runner: &dyn ProcessRunner,
    repo_root: &Path,
) -> Result<Vec<WorktreeEntry>, WorktreeError> {
    let output = runner.run(&list_worktrees_spec(repo_root))?;
    if !output.success {
        return Err(WorktreeError::Git {
            command: "worktree list".to_string(),
            stderr: output.stderr_string().trim().to_string(),
        });
    }
    Ok(parse_worktree_list(&output.stdout))
}

/// Capture the agent worktree's diff against `base` as a [`DiffStat`].
pub fn diff_stat(
    runner: &dyn ProcessRunner,
    worktree: &Path,
    base: &str,
) -> Result<DiffStat, WorktreeError> {
    let output = runner.run(&diff_numstat_spec(worktree, base))?;
    if !output.success {
        return Err(WorktreeError::Git {
            command: "diff --numstat".to_string(),
            stderr: output.stderr_string().trim().to_string(),
        });
    }
    Ok(parse_numstat(&output.stdout))
}

fn run_ok(
    runner: &dyn ProcessRunner,
    spec: &ProcessSpec,
    command: &str,
) -> Result<(), WorktreeError> {
    let output = runner.run(spec)?;
    if output.success {
        Ok(())
    } else {
        Err(WorktreeError::Git {
            command: command.to_string(),
            stderr: output.stderr_string().trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn add_worktree_builds_expected_command() {
        let spec = add_worktree_spec(
            Path::new("/repo"),
            Path::new("/wt/r1-claude"),
            "crew/r1/claude",
            "HEAD",
        );
        assert_eq!(spec.program, "git");
        let args: Vec<_> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "worktree",
                "add",
                "-b",
                "crew/r1/claude",
                "/wt/r1-claude",
                "HEAD"
            ]
        );
        assert_eq!(spec.current_dir, Some(PathBuf::from("/repo")));
    }

    #[test]
    fn create_worktree_succeeds_then_propagates_failure() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            [
                "worktree",
                "add",
                "-b",
                "crew/r1/claude",
                "/wt/r1-claude",
                "HEAD",
            ],
            ok(""),
        );
        create_worktree(
            &runner,
            Path::new("/repo"),
            Path::new("/wt/r1-claude"),
            "crew/r1/claude",
            "HEAD",
        )
        .unwrap();

        // Second call is unscripted → spawn fails with NotFound.
        let unscripted = create_worktree(
            &runner,
            Path::new("/repo"),
            Path::new("/wt/r1-claude"),
            "crew/r1/claude",
            "HEAD",
        );
        assert!(matches!(unscripted, Err(WorktreeError::Spawn(_))));
    }

    #[test]
    fn non_zero_git_is_reported_as_git_error() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            ["worktree", "add", "-b", "b", "/wt/x", "HEAD"],
            err("fatal: branch 'b' already exists"),
        );
        let result = create_worktree(&runner, Path::new("/repo"), Path::new("/wt/x"), "b", "HEAD");
        match result {
            Err(WorktreeError::Git { command, stderr }) => {
                assert_eq!(command, "worktree add");
                assert!(stderr.contains("already exists"));
            }
            other => panic!("expected git error, got {other:?}"),
        }
    }

    #[test]
    fn diff_stat_sums_numstat() {
        let runner = FakeProcessRunner::new();
        runner.expect(
            "git",
            ["diff", "--numstat", "HEAD"],
            ok("3\t1\tsrc/lib.rs\n10\t0\tsrc/new.rs\n-\t-\tassets/logo.png\n"),
        );
        let stat = diff_stat(&runner, Path::new("/wt/r1-claude"), "HEAD").unwrap();
        assert_eq!(stat.files_changed, 3);
        assert_eq!(stat.insertions, 13);
        assert_eq!(stat.deletions, 1);
        assert!(!stat.is_empty());
    }

    #[test]
    fn empty_diff_is_empty_stat() {
        assert!(parse_numstat(b"").is_empty());
        assert!(parse_numstat(b"\n\n").is_empty());
    }

    #[test]
    fn parses_worktree_list_porcelain() {
        let payload = "\
worktree /repo
HEAD abc123
branch refs/heads/main

worktree /wt/r1-claude
HEAD def456
branch refs/heads/crew/r1/claude

worktree /wt/r1-detached
HEAD 999aaa
detached
";
        let entries = parse_worktree_list(payload.as_bytes());
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "/repo");
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].branch.as_deref(), Some("crew/r1/claude"));
        assert_eq!(entries[1].head.as_deref(), Some("def456"));
        assert_eq!(entries[2].branch, None);
        assert_eq!(entries[2].head.as_deref(), Some("999aaa"));
    }
}
