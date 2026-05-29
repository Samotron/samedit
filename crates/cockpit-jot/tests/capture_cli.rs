//! End-to-end test of the `cockpit-jot capture` CLI: it drives the real
//! compiled binary against a tempdir org root + `org.toml`, proving the
//! config → template → `WriteFile` path lands an entry on disk.

use std::path::Path;
use std::process::Command;

/// Path to the binary cargo built for this integration test.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_cockpit-jot")
}

/// Write a root with one `inbox.org` and an `org.toml` declaring a `t` Todo
/// template filed under "Tasks". Returns the tempdir (kept alive by caller).
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("inbox.org"), "* Tasks\n").unwrap();
    let toml = format!(
        "[org]\nroot = \"{root}\"\ndefault_todo_keywords = [\"TODO\", \"DONE\"]\n\n\
         [[org.capture]]\nkey = \"t\"\nname = \"Todo\"\n\
         target = {{ file = \"inbox.org\", under = \"Tasks\" }}\n\
         template = \"* TODO %?\"\n",
        root = dir.path().display()
    );
    std::fs::write(dir.path().join("org.toml"), toml).unwrap();
    dir
}

fn run(config: &Path, args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .expect("spawn cockpit-jot")
}

#[test]
fn capture_writes_entry_to_disk() {
    let dir = fixture();
    let config = dir.path().join("org.toml");

    let out = run(&config, &["capture", "t", "buy", "milk"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Captured to"));

    let inbox = std::fs::read_to_string(dir.path().join("inbox.org")).unwrap();
    assert_eq!(inbox, "* Tasks\n** TODO buy milk\n");
}

#[test]
fn capture_unknown_key_fails_and_lists_templates() {
    let dir = fixture();
    let config = dir.path().join("org.toml");

    let out = run(&config, &["capture", "zzz", "nope"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no capture template with key 'zzz'"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("t (Todo)"),
        "stderr should list available templates: {stderr}"
    );

    // The unknown key must not have touched the file.
    let inbox = std::fs::read_to_string(dir.path().join("inbox.org")).unwrap();
    assert_eq!(inbox, "* Tasks\n");
}

#[test]
fn capture_annotation_flows_into_template() {
    // A %a template picks up --annotate.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.org"), "* Inbox\n").unwrap();
    let toml = format!(
        "[org]\nroot = \"{root}\"\n\n[[org.capture]]\nkey = \"n\"\nname = \"Note\"\n\
         target = {{ file = \"notes.org\", under = \"Inbox\" }}\n\
         template = \"* %? from %a\"\n",
        root = dir.path().display()
    );
    std::fs::write(dir.path().join("org.toml"), &toml).unwrap();

    let out = run(
        &dir.path().join("org.toml"),
        &[
            "capture",
            "n",
            "--annotate",
            "src/lib.rs:42",
            "look",
            "here",
        ],
    );
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let notes = std::fs::read_to_string(dir.path().join("notes.org")).unwrap();
    assert_eq!(notes, "* Inbox\n** look here from src/lib.rs:42\n");
}

#[test]
fn capture_missing_key_reports_usage() {
    let dir = fixture();
    let out = run(&dir.path().join("org.toml"), &["capture"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("capture needs a template key"),);
}
