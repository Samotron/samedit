//! Git status integration (spec §23 v0.3 / M3.4).
//!
//! Shells out to `git status --porcelain=v1 -z` to surface per-file changes
//! the file browser can badge. Best-effort: a missing `git` binary or a
//! non-git directory simply yields an empty list — badges never block the
//! file browser. The parser is pure and headless-testable.

use std::path::{Path, PathBuf};

use crate::env::{ProcessRunner, ProcessSpec, StdProcessRunner};

/// Per-file git status the UI badges with. Distilled from the two-character
/// porcelain code into the categories the file browser actually cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
    Ignored,
}

impl GitStatus {
    /// Single-character badge for the file browser pane.
    pub fn badge(self) -> char {
        match self {
            GitStatus::Modified => 'M',
            GitStatus::Added => 'A',
            GitStatus::Deleted => 'D',
            GitStatus::Renamed => 'R',
            GitStatus::Untracked => '?',
            GitStatus::Conflicted => '!',
            GitStatus::Ignored => 'I',
        }
    }
}

/// Run `git status --porcelain=v1 -z` in `project_root` and return per-path
/// statuses. Returns an empty list when `git` is missing, this is not a git
/// working tree, or the command fails — badges are advisory, never fatal.
///
/// Production wrapper around [`git_status_with`] using the std-backed
/// process runner (M4.10). Tests can call `git_status_with` and pass a
/// fake to avoid actually spawning `git`.
pub fn git_status(project_root: &Path) -> Vec<(PathBuf, GitStatus)> {
    git_status_with(project_root, &StdProcessRunner)
}

/// Trait-injected variant of [`git_status`] — same semantics, but the
/// process spawn goes through `runner` so tests can stub it out (M4.10).
pub fn git_status_with(
    project_root: &Path,
    runner: &dyn ProcessRunner,
) -> Vec<(PathBuf, GitStatus)> {
    let spec = ProcessSpec::new("git")
        .args(["status", "--porcelain=v1", "-z"])
        .current_dir(project_root);
    match runner.run(&spec) {
        Ok(output) if output.success => parse_porcelain_z(&output.stdout),
        _ => Vec::new(),
    }
}

/// Parse a `git status --porcelain=v1 -z` payload. Entries are NUL-terminated:
/// each is `XY <path>`; a rename entry is followed by another NUL-terminated
/// original path (discarded — we badge the new path).
pub fn parse_porcelain_z(bytes: &[u8]) -> Vec<(PathBuf, GitStatus)> {
    let mut out = Vec::new();
    let mut iter = bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());

    while let Some(entry) = iter.next() {
        // Minimum entry shape is `XY <path>` (4 bytes); skip malformed lines.
        if entry.len() < 4 {
            continue;
        }
        let x = entry[0];
        let y = entry[1];
        let path = PathBuf::from(String::from_utf8_lossy(&entry[3..]).into_owned());
        let status = classify(x, y);
        if x == b'R' || y == b'R' {
            // Skip the original path that follows a rename entry.
            iter.next();
        }
        out.push((path, status));
    }
    out
}

fn classify(x: u8, y: u8) -> GitStatus {
    if x == b'?' && y == b'?' {
        return GitStatus::Untracked;
    }
    if x == b'!' && y == b'!' {
        return GitStatus::Ignored;
    }
    if x == b'U' || y == b'U' || (x == b'A' && y == b'A') || (x == b'D' && y == b'D') {
        return GitStatus::Conflicted;
    }
    if x == b'R' || y == b'R' {
        return GitStatus::Renamed;
    }
    if x == b'D' || y == b'D' {
        return GitStatus::Deleted;
    }
    if x == b'A' || y == b'A' {
        return GitStatus::Added;
    }
    // Catches `M`, ` M`, `MM`, `T`, `C` — anything else with an index/worktree
    // mutation is folded into "modified" for badge purposes.
    GitStatus::Modified
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_untracked_and_staged_entries() {
        let input = b" M src/main.rs\0?? new.rs\0M  staged.rs\0";
        assert_eq!(
            parse_porcelain_z(input),
            vec![
                (PathBuf::from("src/main.rs"), GitStatus::Modified),
                (PathBuf::from("new.rs"), GitStatus::Untracked),
                (PathBuf::from("staged.rs"), GitStatus::Modified),
            ]
        );
    }

    #[test]
    fn rename_consumes_and_discards_the_original_path() {
        let input = b"R  new.rs\0old.rs\0 M after.rs\0";
        assert_eq!(
            parse_porcelain_z(input),
            vec![
                (PathBuf::from("new.rs"), GitStatus::Renamed),
                (PathBuf::from("after.rs"), GitStatus::Modified),
            ]
        );
    }

    #[test]
    fn untracked_and_ignored_are_distinguished() {
        let input = b"?? unt.rs\0!! ig.rs\0";
        assert_eq!(
            parse_porcelain_z(input),
            vec![
                (PathBuf::from("unt.rs"), GitStatus::Untracked),
                (PathBuf::from("ig.rs"), GitStatus::Ignored),
            ]
        );
    }

    #[test]
    fn merge_conflicts_classified_regardless_of_orientation() {
        for input in [
            b"UU conflict.rs\0".as_slice(),
            b"AU conflict.rs\0".as_slice(),
            b"AA conflict.rs\0".as_slice(),
            b"DD conflict.rs\0".as_slice(),
        ] {
            assert_eq!(
                parse_porcelain_z(input),
                vec![(PathBuf::from("conflict.rs"), GitStatus::Conflicted)],
                "input: {input:?}",
            );
        }
    }

    #[test]
    fn deleted_files_classified() {
        let input = b" D removed.rs\0D  removed-staged.rs\0";
        assert_eq!(
            parse_porcelain_z(input),
            vec![
                (PathBuf::from("removed.rs"), GitStatus::Deleted),
                (PathBuf::from("removed-staged.rs"), GitStatus::Deleted),
            ]
        );
    }

    #[test]
    fn empty_payload_yields_no_entries() {
        assert!(parse_porcelain_z(b"").is_empty());
    }

    #[test]
    fn badge_letters_are_stable() {
        assert_eq!(GitStatus::Modified.badge(), 'M');
        assert_eq!(GitStatus::Untracked.badge(), '?');
        assert_eq!(GitStatus::Renamed.badge(), 'R');
        assert_eq!(GitStatus::Conflicted.badge(), '!');
    }
}
