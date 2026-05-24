//! Golden tests for [`cockpit_terminal::path_detect`] over the bundled
//! `terminal-output` fixtures (spec §18.3 / §18.10 / M1.19).

use std::fmt::Write;

use cockpit_terminal::path_detect::detect_paths;
use cockpit_testkit::fixture_path;

fn snapshot(fixture: &str) -> String {
    let path = fixture_path("terminal-output").join(fixture);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {fixture}: {e}"));
    let text = text.replace("\r\n", "\n");

    let mut out = String::new();
    writeln!(&mut out, "fixture: {fixture}").unwrap();
    writeln!(&mut out, "matches:").unwrap();
    for m in detect_paths(&text) {
        let line = m.line.map(|n| n.to_string()).unwrap_or_else(|| "-".into());
        let col = m
            .column
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into());
        writeln!(
            &mut out,
            "  - path={} line={} col={} span={}..{}",
            m.path, line, col, m.span.start, m.span.end
        )
        .unwrap();
    }
    out
}

#[test]
fn golden_rust_error() {
    insta::assert_snapshot!(snapshot("rust-error.txt"));
}

#[test]
fn golden_test_failure() {
    insta::assert_snapshot!(snapshot("test-failure.txt"));
}

#[test]
fn golden_python_traceback() {
    insta::assert_snapshot!(snapshot("python-traceback.txt"));
}
