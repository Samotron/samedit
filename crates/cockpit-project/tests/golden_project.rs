//! Golden tests for project & mise extraction (spec §18.3 / M1.19).
//!
//! Project detection runs over the bundled fixtures and `mise.toml` parsing
//! runs over an inline document. Rendering is deliberately path-free and omits
//! `mise.available` (which probes the host `PATH`) so snapshots stay stable
//! across machines and CI runners.

use std::fmt::Write;

use cockpit_project::{MiseProject, ProjectDetection, detect_project, parse_mise_toml};
use cockpit_testkit::fixture_path;

/// Render a detection result without absolute paths or host-dependent fields.
fn render_detection(detection: &ProjectDetection) -> String {
    let mut out = String::new();
    writeln!(out, "display_name: {}", detection.display_name).unwrap();
    writeln!(out, "detected:     {}", detection.detected()).unwrap();
    writeln!(out, "strongest:    {:?}", detection.strongest_signal).unwrap();
    let kinds: Vec<_> = detection.signals.iter().map(|signal| signal.kind).collect();
    writeln!(out, "signals:      {kinds:?}").unwrap();
    out.push_str(&render_mise(&detection.mise));
    out
}

/// Render parsed mise data without absolute paths or host-dependent fields.
fn render_mise(mise: &MiseProject) -> String {
    let mut out = String::new();
    writeln!(out, "mise.detected: {}", mise.detected).unwrap();
    let config = mise
        .config_path
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str());
    writeln!(out, "mise.config:   {config:?}").unwrap();

    writeln!(out, "mise.tools:").unwrap();
    for tool in &mise.tools {
        writeln!(out, "  {} = {}", tool.name, tool.version).unwrap();
    }
    writeln!(out, "mise.tasks:").unwrap();
    for task in &mise.tasks {
        writeln!(
            out,
            "  {} | desc={:?} | run={:?}",
            task.name, task.description, task.run
        )
        .unwrap();
    }
    writeln!(out, "mise.env:").unwrap();
    for var in &mise.env {
        writeln!(out, "  {} = {}", var.name, var.value).unwrap();
    }
    writeln!(out, "mise.metadata: {:?}", mise.metadata).unwrap();
    out
}

#[test]
fn golden_detect_rust_basic() {
    let detection = detect_project(fixture_path("rust-basic")).expect("detect rust-basic");
    insta::assert_snapshot!(render_detection(&detection));
}

#[test]
fn golden_detect_mise_basic() {
    let detection = detect_project(fixture_path("mise-basic")).expect("detect mise-basic");
    insta::assert_snapshot!(render_detection(&detection));
}

#[test]
fn golden_detect_file_tree_git_project() {
    // A `.git` directory cannot be committed inside a fixture — Git refuses to
    // track nested `.git` entries — so build the Git-marked project on disk.
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path().join("file-tree");
    std::fs::create_dir(&root).expect("create project dir");
    std::fs::create_dir(root.join(".git")).expect("create .git dir");

    let detection = detect_project(&root).expect("detect file-tree");
    insta::assert_snapshot!(render_detection(&detection));
}

#[test]
fn golden_parse_mise_toml() {
    let input = r#"
[tools]
node = "22.0.0"
rust = "1.95.0"

[env]
RUST_LOG = "debug"

[tasks.build]
description = "Build the workspace"
run = "cargo build --workspace"

[tasks.test]
run = "cargo test"

[metadata.cockpit]
name = "Sample Project"
default_task = "test"
terminal_workspace = "zellij"
"#;
    let mise = parse_mise_toml(input).expect("parse mise toml");
    insta::assert_snapshot!(render_mise(&mise));
}
