//! Format-on-save planning (M4.4).
//!
//! Pure, headless decision about *how* to format the currently open document.
//! The spec ([`IMPLEMENTATION_PLAN.md`] M4.4) is unambiguous: **mise task
//! wins, always**. If the project defines a `format` (or `format:<lang>`)
//! task in `mise.toml`, cockpit runs that. Otherwise we look for a known
//! formatter (`rustfmt`, `prettier`, `ruff`, `black`, `sqlfluff`) declared in
//! `[tools]` or on `$PATH` — when found, we surface a prompt offering to add
//! a `format` task to `mise.toml` (AGENTS.md hard rule #6: detect, surface,
//! prompt — never silently install or modify). If neither path is available,
//! we fall back to LSP `textDocument/formatting` so language servers that
//! ship a formatter still do useful work.
//!
//! [`IMPLEMENTATION_PLAN.md`]: ../../../../IMPLEMENTATION_PLAN.md

use crate::MiseProject;

/// Formatters cockpit knows how to detect and suggest. Adding a new one
/// means extending [`KnownFormatter::for_language_id`] and the
/// `mise.toml [tasks.format]` template returned by
/// [`KnownFormatter::default_run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownFormatter {
    /// Rust's official formatter; ships with rustup.
    Rustfmt,
    /// JavaScript/TypeScript formatter (also handles JSON, CSS, Markdown).
    Prettier,
    /// Ruff's `format` command — preferred over `black` when both exist
    /// because it covers `isort` too and is the modern Python default.
    Ruff,
    /// Python formatter; used when `ruff` is absent.
    Black,
    /// SQL formatter; pairs with `sqls` (LSP) from M4.8.
    Sqlfluff,
    /// `goimports` — preferred Go formatter; rewrites imports and formats
    /// (a strict superset of `gofmt`).
    Goimports,
    /// Go's standard formatter; bundled with every Go toolchain.
    Gofmt,
}

impl KnownFormatter {
    /// Binary name as it would appear on `$PATH` or in `mise.toml [tools]`.
    pub fn binary(self) -> &'static str {
        match self {
            Self::Rustfmt => "rustfmt",
            Self::Prettier => "prettier",
            Self::Ruff => "ruff",
            Self::Black => "black",
            Self::Sqlfluff => "sqlfluff",
            Self::Goimports => "goimports",
            Self::Gofmt => "gofmt",
        }
    }

    /// The default formatter for a given LSP `languageId`, in priority order.
    ///
    /// Mirrors the formatter list in [`IMPLEMENTATION_PLAN.md`] M4.4. The
    /// Python entry returns `ruff` first; callers should fall back to
    /// [`KnownFormatter::Black`] when `ruff` is neither in `[tools]` nor on
    /// `$PATH` — see [`plan_format`] for the canonical handling.
    ///
    /// [`IMPLEMENTATION_PLAN.md`]: ../../../../IMPLEMENTATION_PLAN.md
    pub fn for_language_id(language_id: &str) -> &'static [KnownFormatter] {
        match language_id {
            "rust" => &[KnownFormatter::Rustfmt],
            "typescript" | "javascript" | "json" | "css" | "markdown" => {
                &[KnownFormatter::Prettier]
            }
            "python" => &[KnownFormatter::Ruff, KnownFormatter::Black],
            "sql" => &[KnownFormatter::Sqlfluff],
            "go" => &[KnownFormatter::Goimports, KnownFormatter::Gofmt],
            _ => &[],
        }
    }

    /// The `run = "..."` line cockpit suggests for a new `[tasks.format]`
    /// entry. `{{file}}` is left as a literal placeholder so users can edit
    /// the task to taste; the format command itself reads the file path from
    /// the first positional argument cockpit passes (`mise run format -- <path>`).
    pub fn default_run(self) -> &'static str {
        match self {
            Self::Rustfmt => "rustfmt \"$1\"",
            Self::Prettier => "prettier --write \"$1\"",
            Self::Ruff => "ruff format \"$1\"",
            Self::Black => "black \"$1\"",
            Self::Sqlfluff => "sqlfluff format \"$1\"",
            Self::Goimports => "goimports -w \"$1\"",
            Self::Gofmt => "gofmt -w \"$1\"",
        }
    }
}

/// How cockpit can detect a formatter binary is available.
pub trait BinaryLookup {
    /// True when `binary` is available to spawn (typically `$PATH`).
    fn exists(&self, binary: &str) -> bool;
}

/// `BinaryLookup` that pretends nothing is on `$PATH`. Useful in tests and
/// when callers only care about `mise.toml [tools]` declarations.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBinaryLookup;

impl BinaryLookup for NoBinaryLookup {
    fn exists(&self, _binary: &str) -> bool {
        false
    }
}

/// `BinaryLookup` that searches the real `$PATH`. The production code in
/// `cockpit/src/app.rs` passes this; tests pass [`FixedBinaryLookup`] so the
/// outcome stays deterministic.
#[derive(Debug, Clone, Copy, Default)]
pub struct PathBinaryLookup;

impl BinaryLookup for PathBinaryLookup {
    fn exists(&self, binary: &str) -> bool {
        binary_in_path(binary)
    }
}

fn binary_in_path(binary: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exe_suffix = std::env::consts::EXE_SUFFIX;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if candidate.is_file() {
            return true;
        }
        if !exe_suffix.is_empty() {
            let with_suffix = dir.join(format!("{binary}{exe_suffix}"));
            if with_suffix.is_file() {
                return true;
            }
        }
    }
    false
}

/// `BinaryLookup` backed by a fixed allow-list — handy for tests that need
/// to simulate "rustfmt is on PATH" without touching the real filesystem.
#[derive(Debug, Clone, Default)]
pub struct FixedBinaryLookup {
    allowed: Vec<String>,
}

impl FixedBinaryLookup {
    /// New lookup that reports any of `names` as present and everything else
    /// as absent.
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            allowed: names.into_iter().map(Into::into).collect(),
        }
    }
}

impl BinaryLookup for FixedBinaryLookup {
    fn exists(&self, binary: &str) -> bool {
        self.allowed.iter().any(|name| name == binary)
    }
}

/// The cockpit chose path for formatting the currently open document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatPlan {
    /// Run an existing mise task — `mise run <name>` (spec: mise wins).
    MiseTask {
        /// Task name as declared in `mise.toml [tasks]`.
        name: String,
    },
    /// No mise task, but a known formatter is detectable on PATH or in
    /// `[tools]`. Surface a prompt before doing anything else
    /// (AGENTS.md rule #6) and offer to add a `[tasks.format]` entry.
    SuggestMiseTask {
        /// The formatter cockpit found.
        formatter: KnownFormatter,
        /// True when the formatter is in `mise.toml [tools]` (preferred);
        /// false when it was only found on `$PATH`.
        from_mise_tools: bool,
        /// Suggested `run = "..."` value for the new `[tasks.format]` entry.
        suggested_run: String,
    },
    /// No mise task and no known formatter — the LSP server's
    /// `textDocument/formatting` is the only remaining option. The caller
    /// decides whether the active server actually advertises the capability.
    LspOnly,
}

/// Decide how to format an open file in `project` whose LSP `languageId` is
/// `language_id` (e.g. `"rust"`, `"python"`).
///
/// Mise tasks beat formatter detection — see [`FormatPlan`] for the rules.
/// `lookup` is the seam for testability and platform abstraction; production
/// callers pass a `$PATH`-aware impl, tests use [`FixedBinaryLookup`] or
/// [`NoBinaryLookup`].
pub fn plan_format(
    project: &MiseProject,
    language_id: Option<&str>,
    lookup: &dyn BinaryLookup,
) -> FormatPlan {
    if let Some(task) = mise_format_task(project, language_id) {
        return FormatPlan::MiseTask { name: task };
    }
    let Some(language_id) = language_id else {
        return FormatPlan::LspOnly;
    };
    for &formatter in KnownFormatter::for_language_id(language_id) {
        let from_mise_tools = project
            .tools
            .iter()
            .any(|tool| tool.name == formatter.binary());
        if from_mise_tools || lookup.exists(formatter.binary()) {
            return FormatPlan::SuggestMiseTask {
                formatter,
                from_mise_tools,
                suggested_run: formatter.default_run().to_string(),
            };
        }
    }
    FormatPlan::LspOnly
}

/// Resolve a `format` or `format:<language_id>` task in `project`. The
/// language-specific name wins when both exist so a project can ship one
/// task per language without ambiguity.
fn mise_format_task(project: &MiseProject, language_id: Option<&str>) -> Option<String> {
    let lang_suffix = language_id.map(|id| format!("format:{id}"));
    if let Some(suffix) = lang_suffix.as_deref()
        && let Some(task) = project.tasks.iter().find(|task| task.name == suffix)
    {
        return Some(task.name.clone());
    }
    project
        .tasks
        .iter()
        .find(|task| task.name == "format")
        .map(|task| task.name.clone())
}

/// Render the snippet cockpit writes when the user confirms the
/// "Add `format` task to `mise.toml`?" prompt. Always appended — never
/// merged into an existing `[tasks.format]` (the planner only suggests it
/// when no `format` task exists).
pub fn render_format_task_snippet(run: &str) -> String {
    format!(
        "\n[tasks.format]\ndescription = \"Format a file in this project\"\nrun = {}\n",
        toml_quote(run)
    )
}

fn toml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Task, Tool};

    fn project_with_tasks(tasks: Vec<&str>) -> MiseProject {
        MiseProject {
            tasks: tasks
                .into_iter()
                .map(|name| Task {
                    name: name.to_string(),
                    description: None,
                    run: Some("echo".to_string()),
                })
                .collect(),
            ..MiseProject::default()
        }
    }

    fn project_with_tools(tools: Vec<&str>) -> MiseProject {
        MiseProject {
            tools: tools
                .into_iter()
                .map(|name| Tool {
                    name: name.to_string(),
                    version: "latest".to_string(),
                })
                .collect(),
            ..MiseProject::default()
        }
    }

    #[test]
    fn mise_format_task_wins_over_path_detection() {
        let project = project_with_tasks(vec!["format", "lint"]);
        let plan = plan_format(&project, Some("rust"), &FixedBinaryLookup::new(["rustfmt"]));
        assert_eq!(
            plan,
            FormatPlan::MiseTask {
                name: "format".to_string()
            }
        );
    }

    #[test]
    fn format_language_specific_task_wins_over_generic_format() {
        let project = project_with_tasks(vec!["format", "format:rust"]);
        let plan = plan_format(&project, Some("rust"), &NoBinaryLookup);
        assert_eq!(
            plan,
            FormatPlan::MiseTask {
                name: "format:rust".to_string()
            }
        );
    }

    #[test]
    fn detected_in_mise_tools_suggests_adding_a_format_task() {
        let project = project_with_tools(vec!["rust", "rustfmt"]);
        let plan = plan_format(&project, Some("rust"), &NoBinaryLookup);
        assert_eq!(
            plan,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Rustfmt,
                from_mise_tools: true,
                suggested_run: "rustfmt \"$1\"".to_string(),
            }
        );
    }

    #[test]
    fn detected_on_path_suggests_adding_a_format_task() {
        let project = MiseProject::default();
        let plan = plan_format(
            &project,
            Some("typescript"),
            &FixedBinaryLookup::new(["prettier"]),
        );
        assert_eq!(
            plan,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Prettier,
                from_mise_tools: false,
                suggested_run: "prettier --write \"$1\"".to_string(),
            }
        );
    }

    #[test]
    fn python_prefers_ruff_then_black() {
        let ruff_only = plan_format(
            &MiseProject::default(),
            Some("python"),
            &FixedBinaryLookup::new(["ruff", "black"]),
        );
        assert!(matches!(
            ruff_only,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Ruff,
                ..
            }
        ));

        let black_only = plan_format(
            &MiseProject::default(),
            Some("python"),
            &FixedBinaryLookup::new(["black"]),
        );
        assert!(matches!(
            black_only,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Black,
                ..
            }
        ));
    }

    #[test]
    fn nothing_detectable_falls_back_to_lsp_only() {
        let plan = plan_format(&MiseProject::default(), Some("rust"), &NoBinaryLookup);
        assert_eq!(plan, FormatPlan::LspOnly);
    }

    #[test]
    fn unknown_language_with_no_format_task_falls_back_to_lsp_only() {
        let plan = plan_format(
            &MiseProject::default(),
            Some("haskell"),
            &FixedBinaryLookup::new(["rustfmt", "prettier"]),
        );
        assert_eq!(plan, FormatPlan::LspOnly);
    }

    #[test]
    fn unknown_language_with_a_generic_format_task_uses_the_task() {
        let project = project_with_tasks(vec!["format"]);
        let plan = plan_format(&project, Some("haskell"), &NoBinaryLookup);
        assert_eq!(
            plan,
            FormatPlan::MiseTask {
                name: "format".to_string()
            }
        );
    }

    #[test]
    fn missing_language_id_still_picks_up_a_format_task() {
        let project = project_with_tasks(vec!["format"]);
        let plan = plan_format(&project, None, &NoBinaryLookup);
        assert_eq!(
            plan,
            FormatPlan::MiseTask {
                name: "format".to_string()
            }
        );
    }

    #[test]
    fn go_prefers_goimports_then_gofmt() {
        let goimports_only = plan_format(
            &MiseProject::default(),
            Some("go"),
            &FixedBinaryLookup::new(["goimports", "gofmt"]),
        );
        assert!(matches!(
            goimports_only,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Goimports,
                ..
            }
        ));

        let gofmt_only = plan_format(
            &MiseProject::default(),
            Some("go"),
            &FixedBinaryLookup::new(["gofmt"]),
        );
        assert!(matches!(
            gofmt_only,
            FormatPlan::SuggestMiseTask {
                formatter: KnownFormatter::Gofmt,
                ..
            }
        ));
    }

    #[test]
    fn format_task_snippet_is_valid_toml_for_typical_run_lines() {
        for formatter in [
            KnownFormatter::Rustfmt,
            KnownFormatter::Prettier,
            KnownFormatter::Ruff,
            KnownFormatter::Black,
            KnownFormatter::Sqlfluff,
            KnownFormatter::Goimports,
            KnownFormatter::Gofmt,
        ] {
            let snippet = render_format_task_snippet(formatter.default_run());
            let parsed: toml::Value = toml::from_str(&snippet).expect("snippet must be valid TOML");
            let task = parsed
                .get("tasks")
                .and_then(|tasks| tasks.get("format"))
                .expect("[tasks.format] table");
            let run = task.get("run").and_then(|r| r.as_str()).unwrap_or("");
            assert!(
                run.contains(formatter.binary()),
                "run line should mention the formatter binary: {run}"
            );
        }
    }

    #[test]
    fn snippet_quoting_escapes_quotes_and_backslashes() {
        let snippet = render_format_task_snippet(r#"echo "hi" \n done"#);
        assert!(
            snippet.contains(r#"run = "echo \"hi\" \\n done""#),
            "{snippet}"
        );
        let _: toml::Value = toml::from_str(&snippet).expect("must remain valid TOML");
    }
}
