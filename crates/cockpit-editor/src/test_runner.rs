//! Test-runner command construction (v0.10 M10.5).
//!
//! Pure helpers that turn a (language, scope, file, test name) tuple into the
//! argv the binary types into a terminal pane. The mise-task contract still
//! wins when one is defined — these helpers cover the fallback path for
//! languages whose toolchain ships its own test runner (just Go today; Rust
//! still goes through `cargo nextest run` via mise).
//!
//! Headless and language-aware so the binary's `Test: Run *` dispatch can
//! call into a single seam regardless of the active document.

use std::path::Path;

use crate::highlight::Language;

/// Which scope the user invoked from `Test: Run *`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestScope {
    /// All tests in the project.
    All,
    /// Every test in the currently-open file.
    CurrentFile,
    /// The single test the cursor is in (or before).
    Nearest,
}

/// Build the argv for running `scope` against `relative_path` (a
/// project-relative file path) and optional `test_name` in `language`. Returns
/// `None` for languages that don't have a native fallback — those still go
/// through the mise `test` task in the binary.
///
/// Path handling: callers pass paths relative to the project root using
/// forward slashes (`render_document_path` already normalises this on
/// Windows). The Go `./...` and `./pkg/foo` shapes need the leading `./`
/// so `go` treats them as package paths instead of import paths.
pub fn fallback_test_command(
    language: Language,
    scope: TestScope,
    relative_path: &Path,
    test_name: Option<&str>,
) -> Option<Vec<String>> {
    match language {
        Language::Go => Some(go_test_command(scope, relative_path, test_name)),
        _ => None,
    }
}

fn go_test_command(scope: TestScope, relative_path: &Path, test_name: Option<&str>) -> Vec<String> {
    let package = go_package_arg(relative_path);
    match scope {
        TestScope::All => vec!["go".into(), "test".into(), "./...".into()],
        TestScope::CurrentFile => vec!["go".into(), "test".into(), package],
        TestScope::Nearest => {
            let name = test_name.unwrap_or("");
            let pattern = if name.is_empty() {
                ".".to_string()
            } else {
                format!("^{name}$")
            };
            vec!["go".into(), "test".into(), "-run".into(), pattern, package]
        }
    }
}

/// Project-relative directory of `relative_path`, formatted as a Go package
/// argument (`./pkg/foo`). Files at the project root collapse to `.`.
fn go_package_arg(relative_path: &Path) -> String {
    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    let raw = parent.to_string_lossy().replace('\\', "/");
    if raw.is_empty() {
        ".".to_string()
    } else {
        format!("./{raw}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(path: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(path)
    }

    #[test]
    fn non_go_languages_have_no_fallback_command() {
        for language in [
            Language::Rust,
            Language::Python,
            Language::TypeScript,
            Language::Sql,
            Language::Ggsql,
        ] {
            assert_eq!(
                fallback_test_command(language, TestScope::All, &p("a.rs"), None),
                None,
                "{language:?} should defer to the mise task path",
            );
        }
    }

    #[test]
    fn go_run_all_walks_every_package() {
        let cmd =
            fallback_test_command(Language::Go, TestScope::All, &p("main_test.go"), None).unwrap();
        assert_eq!(cmd, ["go", "test", "./..."]);
    }

    #[test]
    fn go_run_current_file_targets_the_files_package() {
        let cmd = fallback_test_command(
            Language::Go,
            TestScope::CurrentFile,
            &p("pkg/widgets/widget_test.go"),
            None,
        )
        .unwrap();
        assert_eq!(cmd, ["go", "test", "./pkg/widgets"]);
    }

    #[test]
    fn go_run_current_file_on_a_root_file_collapses_to_dot_package() {
        let cmd = fallback_test_command(
            Language::Go,
            TestScope::CurrentFile,
            &p("main_test.go"),
            None,
        )
        .unwrap();
        assert_eq!(cmd, ["go", "test", "."]);
    }

    #[test]
    fn go_run_nearest_uses_anchored_run_pattern_against_the_files_package() {
        let cmd = fallback_test_command(
            Language::Go,
            TestScope::Nearest,
            &p("pkg/widgets/widget_test.go"),
            Some("TestAlpha"),
        )
        .unwrap();
        assert_eq!(cmd, ["go", "test", "-run", "^TestAlpha$", "./pkg/widgets"]);
    }

    #[test]
    fn go_run_nearest_without_a_name_falls_back_to_dot_pattern() {
        let cmd = fallback_test_command(Language::Go, TestScope::Nearest, &p("main_test.go"), None)
            .unwrap();
        assert_eq!(cmd, ["go", "test", "-run", ".", "."]);
    }

    #[test]
    #[cfg(windows)]
    fn go_package_arg_normalises_windows_separators() {
        // On Windows, `Path::parent` already splits on `\`; the join step
        // then re-renders with `/` so the `go` CLI accepts the package arg.
        let cmd = fallback_test_command(
            Language::Go,
            TestScope::CurrentFile,
            &p("pkg\\widgets\\widget_test.go"),
            None,
        )
        .unwrap();
        assert_eq!(cmd, ["go", "test", "./pkg/widgets"]);
    }
}
