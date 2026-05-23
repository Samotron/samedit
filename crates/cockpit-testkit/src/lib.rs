//! `cockpit-testkit` — shared test support and fixture helpers.
//!
//! Locating the bundled fixtures under [`tests/fixtures/`](../../tests/fixtures)
//! works from both unit tests in any crate and from the `cockpit` binary's
//! `--fixture` dev mode (M1.21). Tempdir builders, fake FS / process / clock
//! impls, and golden-file helpers will land here as the milestones that need
//! them arrive.

pub mod bench;

pub use bench::{PhaseMeasurement, format_measurements, measure_phase, total};

use std::path::{Path, PathBuf};

/// Absolute path to the workspace's `tests/fixtures/` directory.
///
/// Resolved at compile time from this crate's `CARGO_MANIFEST_DIR`, so the
/// returned path is stable across `cargo test`, `cargo run`, and the
/// `cargo run -- --fixture <name>` developer flow described in spec §18.12.
pub fn fixtures_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent() // crates/
        .and_then(Path::parent) // workspace root
        .expect("cockpit-testkit lives at <root>/crates/cockpit-testkit")
        .join("tests")
        .join("fixtures")
}

/// Resolve a named bundled fixture, e.g. `rust-basic` or `mise-basic`.
pub fn fixture_path(name: &str) -> PathBuf {
    fixtures_root().join(name)
}

/// Names of the fixtures committed under [`tests/fixtures/`](../../tests/fixtures).
pub const BUILTIN_FIXTURES: &[&str] = &["rust-basic", "mise-basic", "file-tree", "terminal-output"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_root_points_at_workspace_tests_dir() {
        let root = fixtures_root();
        assert!(
            root.ends_with("tests/fixtures"),
            "unexpected fixtures root: {}",
            root.display()
        );
        assert!(root.is_dir(), "fixtures dir missing: {}", root.display());
    }

    #[test]
    fn every_builtin_fixture_exists_on_disk() {
        for name in BUILTIN_FIXTURES {
            let path = fixture_path(name);
            assert!(path.is_dir(), "missing fixture: {}", path.display());
        }
    }
}
