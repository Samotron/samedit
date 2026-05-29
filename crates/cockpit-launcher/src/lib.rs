//! `cockpit-launcher` — headless core for the v0.13 quick-action launcher
//! (Raycast-style), milestone M13.1.
//!
//! A system-wide, hotkey-summoned launcher that fuzzy-searches across action
//! *providers* and dispatches the chosen [`Action`]. This crate is the
//! backend-free brain: the provider trait, the `nucleo`-backed matcher, the
//! multi-provider merge, and the typed action/dispatch model. It owns no
//! window, no tray, no hotkey, and no IPC — those live in the `cockpit-quick`
//! sibling binary (M13.5), exactly as `cockpit-jot`'s headless `JotController`
//! sits under its winit shell.
//!
//! # Providers
//!
//! - [`mise::MiseTasksProvider`] (M13.2) — every task across registered mise
//!   projects; Enter spawns `mise run <task>` in the project root.
//! - [`builtins::CalculatorProvider`] / [`builtins::UrlProvider`] (M13.4,
//!   headless subset) — turn the query itself into an action.
//! - Lua actions (M13.3) and the IPC-backed built-ins (`Open Project`,
//!   `Switch Theme`, `Org Capture`/`Agenda`) land with the binary, where a
//!   live Lua runtime / IPC client exists. [`action::ActionRun::Lua`] already
//!   models the Lua dispatch path so the enum is closed.
//!
//! # Execution spine
//!
//! Every [`action::ActionRun`] resolves to a `cockpit-commands` dispatch in
//! the end (AGENTS §2 #5) — the open/process/lua variants are conveniences
//! the binary lowers onto the matching command, so the launcher and the
//! in-cockpit palette never diverge.
//!
//! Headless and unit-tested per the AGENTS.md hard rules — no window, no GPU,
//! no PTY, no real filesystem (tests use `cockpit-project`'s `FakeFileSystem`),
//! no network.

pub mod action;
pub mod builtins;
pub mod calc;
pub mod launcher;
pub mod lua;
pub mod mise;
pub mod provider;
pub mod service;

pub use action::{Action, ActionArg, ActionIcon, ActionRun, LuaActionHandle};
pub use builtins::{
    CLIPBOARD_COPY_COMMAND, CalculatorProvider, ORG_AGENDA_COMMAND, ORG_CAPTURE_COMMAND,
    OrgProvider, RecentProjectsProvider, THEME_SWITCH_COMMAND, ThemeProvider, UrlProvider,
};
pub use launcher::{DEFAULT_MAX_ROWS, Launcher, RankedAction};
pub use lua::{LuaAction, LuaActionsProvider};
pub use mise::MiseTasksProvider;
pub use provider::{ActionProvider, DEFAULT_PROVIDER_QUOTA};
pub use service::{CockpitOutcome, CockpitRequest, CockpitResponse};

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic provider returning a fixed action list, fuzzy-filtered by
    /// the launcher. Lets the ranking/quota tests stay independent of any
    /// real provider.
    struct StaticProvider {
        id: &'static str,
        titles: Vec<&'static str>,
        quota: usize,
    }

    impl StaticProvider {
        fn new(id: &'static str, titles: Vec<&'static str>) -> Self {
            Self {
                id,
                titles,
                quota: DEFAULT_PROVIDER_QUOTA,
            }
        }

        fn with_quota(mut self, quota: usize) -> Self {
            self.quota = quota;
            self
        }
    }

    impl ActionProvider for StaticProvider {
        fn id(&self) -> &str {
            self.id
        }

        fn quota(&self) -> usize {
            self.quota
        }

        fn search(&self, _query: &str) -> Vec<Action> {
            self.titles
                .iter()
                .map(|title| {
                    Action::new(
                        format!("{}:{title}", self.id),
                        *title,
                        ActionRun::OpenUrl(String::new()),
                    )
                })
                .collect()
        }
    }

    #[test]
    fn empty_query_lists_favourites_from_every_provider() {
        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(StaticProvider::new("a", vec!["one", "two"])))
            .register(Box::new(StaticProvider::new("b", vec!["three"])));

        let results = launcher.search("");
        assert_eq!(results.len(), 3);
        // Empty query → score 0 everywhere → deterministic title order.
        let titles: Vec<&str> = results.iter().map(|r| r.action.title.as_str()).collect();
        assert_eq!(titles, vec!["one", "three", "two"]);
    }

    #[test]
    fn fuzzy_query_filters_and_ranks_across_providers() {
        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(StaticProvider::new(
                "tasks",
                vec!["run tests", "build", "test ui"],
            )))
            .register(Box::new(StaticProvider::new("docs", vec!["testing guide"])));

        let results = launcher.search("test");
        // "build" doesn't contain the subsequence t-e-s-t; everything else does.
        assert!(results.iter().all(|r| r.action.title != "build"));
        assert_eq!(results.len(), 3);
        // The best match leads.
        assert!(!results.is_empty());
    }

    #[test]
    fn per_provider_quota_caps_one_chatty_provider() {
        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(
                StaticProvider::new("chatty", vec!["a1", "a2", "a3", "a4", "a5"]).with_quota(2),
            ))
            .register(Box::new(StaticProvider::new("calm", vec!["b1"])));

        let results = launcher.search("");
        let chatty = results.iter().filter(|r| r.provider == "chatty").count();
        let calm = results.iter().filter(|r| r.provider == "calm").count();
        assert_eq!(chatty, 2, "chatty provider capped to its quota");
        assert_eq!(calm, 1);
    }

    #[test]
    fn max_rows_caps_the_merged_list() {
        let mut launcher = Launcher::new().with_max_rows(3);
        launcher.register(Box::new(
            StaticProvider::new("p", vec!["a", "b", "c", "d", "e"]).with_quota(10),
        ));
        assert_eq!(launcher.search("").len(), 3);
    }

    #[test]
    fn verbatim_provider_outranks_fuzzy_matches() {
        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(StaticProvider::new("tasks", vec!["=2+2 task"])))
            .register(Box::new(CalculatorProvider));

        // The query fuzzy-matches the task title, but the calculator's
        // verbatim result must lead.
        let results = launcher.search("=2+2");
        assert_eq!(results[0].provider, "calculator");
        assert_eq!(results[0].action.title, "4");
    }

    #[test]
    fn query_with_special_chars_does_not_panic() {
        let mut launcher = Launcher::new();
        launcher.register(Box::new(StaticProvider::new("p", vec!["normal"])));
        // nucleo treats some chars specially; none of these should panic or
        // match the plain candidate.
        for query in ["^", "$", "'exact", "!neg", "\\", "  "] {
            let _ = launcher.search(query);
        }
    }

    #[test]
    fn end_to_end_with_real_providers() {
        use cockpit_project::env::FakeFileSystem;

        let fs = FakeFileSystem::new();
        fs.insert_file(
            "/code/app/mise.toml",
            "[tasks.test]\nrun = \"cargo test\"\n[tasks.test-ui]\nrun = \"npm test\"\n",
        );

        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(CalculatorProvider))
            .register(Box::new(UrlProvider))
            .register(Box::new(MiseTasksProvider::from_projects(
                &fs,
                ["/code/app"],
            )));

        // mise tasks fuzzy-match.
        let results = launcher.search("test");
        assert!(
            results
                .iter()
                .any(|r| r.provider == "mise" && r.action.title.contains("test"))
        );

        // calculator wins on `=`.
        let calc = launcher.search("=10/4");
        assert_eq!(calc[0].action.title, "2.5");

        // a URL produces an open action.
        let url = launcher.search("https://example.com");
        assert!(
            url.iter().any(
                |r| matches!(&r.action.run, ActionRun::OpenUrl(u) if u == "https://example.com")
            )
        );
    }
}
