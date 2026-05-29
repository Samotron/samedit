//! Built-in providers that need no IPC or external state (M13.4, headless
//! subset): the calculator and the URL opener.
//!
//! The other built-ins from the plan — `Open Project`, `Switch Theme`,
//! `Org Capture`, `Org Agenda` — dispatch over the `cockpit` / `org` IPC
//! services, so they land with the `cockpit-quick` binary (M13.5/M13.7) where
//! a live IPC client exists. These two are self-contained: they turn the
//! query itself into an action.

use std::path::PathBuf;

use cockpit_commands::CommandId;

use crate::action::{Action, ActionArg, ActionIcon, ActionRun};
use crate::calc;
use crate::provider::ActionProvider;

/// `=<expr>` → an action whose Enter copies the computed result to the
/// clipboard. Verbatim (not fuzzy-filtered): the single result floats to the
/// top, Raycast-style.
#[derive(Debug, Default, Clone, Copy)]
pub struct CalculatorProvider;

/// Command the calculator action dispatches: copy a string to the clipboard.
/// The binary owns the real clipboard write; the launcher only carries the
/// value, so the calculator stays on the single command spine.
pub const CLIPBOARD_COPY_COMMAND: &str = "clipboard.copy";

impl ActionProvider for CalculatorProvider {
    fn id(&self) -> &str {
        "calculator"
    }

    fn fuzzy_filtered(&self) -> bool {
        false
    }

    fn quota(&self) -> usize {
        1
    }

    fn search(&self, query: &str) -> Vec<Action> {
        let Some(expr) = query.strip_prefix('=') else {
            return Vec::new();
        };
        let Some(value) = calc::evaluate(expr) else {
            return Vec::new();
        };
        let result = calc::format_result(value);
        vec![
            Action::new(
                "calculator.result",
                result.clone(),
                ActionRun::Command(
                    CommandId::new(CLIPBOARD_COPY_COMMAND),
                    vec![ActionArg::str(result.clone())],
                ),
            )
            .with_subtitle(format!("Press Enter to copy `{result}` to clipboard"))
            .with_icon(ActionIcon::Calculator),
        ]
    }
}

/// A pasted URL → an "Open in browser" action. Verbatim, like the calculator.
#[derive(Debug, Default, Clone, Copy)]
pub struct UrlProvider;

impl ActionProvider for UrlProvider {
    fn id(&self) -> &str {
        "url"
    }

    fn fuzzy_filtered(&self) -> bool {
        false
    }

    fn quota(&self) -> usize {
        1
    }

    fn search(&self, query: &str) -> Vec<Action> {
        if !looks_like_url(query) {
            return Vec::new();
        }
        vec![
            Action::new(
                "url.open",
                format!("Open {query} in browser"),
                ActionRun::OpenUrl(query.to_string()),
            )
            .with_subtitle(query.to_string())
            .with_icon(ActionIcon::Url),
        ]
    }
}

/// Cheap, dependency-free URL sniff: an `http(s)://` scheme followed by a
/// non-empty, space-free host. Deliberately conservative — a real browser
/// will reject anything we wave through, and we never want a stray word like
/// `https` on its own to masquerade as a link.
fn looks_like_url(query: &str) -> bool {
    let rest = query
        .strip_prefix("https://")
        .or_else(|| query.strip_prefix("http://"));
    let Some(rest) = rest else {
        return false;
    };
    // A host must exist, contain a dot (or be `localhost`), and have no
    // whitespace anywhere in the URL.
    if rest.is_empty() || query.chars().any(char::is_whitespace) {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host == "localhost" || host.starts_with("localhost:") || host.contains('.')
}

// ---------------------------------------------------------------------------
// IPC-backed built-ins (M13.4). These are data-driven and headless: the
// binary supplies the data (recent projects over the `cockpit` IPC service,
// theme names from config, capture templates from `org.toml`) and lowers each
// emitted `ActionRun` onto the matching IPC send. The providers themselves do
// no IPC, so they stay unit-testable.
// ---------------------------------------------------------------------------

/// Command id the theme actions dispatch (lowered onto a `cockpit`-service
/// `DispatchCommand` by the binary).
pub const THEME_SWITCH_COMMAND: &str = "theme.switch";
/// Command id an Org capture action dispatches (routed to the `org` service).
pub const ORG_CAPTURE_COMMAND: &str = "org.capture";
/// Command id the Org agenda action dispatches.
pub const ORG_AGENDA_COMMAND: &str = "org.agenda";

/// `Open Project: <name>` entries from the cockpit's recent-projects cache.
/// Enter opens (or focuses) the project; the binary lowers the `OpenPath` run
/// onto a `cockpit`-service `OpenProject` message.
#[derive(Debug, Default, Clone)]
pub struct RecentProjectsProvider {
    projects: Vec<(String, PathBuf)>,
}

impl RecentProjectsProvider {
    /// Build from `(name, root)` pairs, most-recent-first as the cache
    /// supplies them.
    pub fn new(projects: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        Self {
            projects: projects.into_iter().collect(),
        }
    }
}

impl ActionProvider for RecentProjectsProvider {
    fn id(&self) -> &str {
        "projects"
    }

    fn search(&self, _query: &str) -> Vec<Action> {
        self.projects
            .iter()
            .map(|(name, path)| {
                Action::new(
                    format!("project:{name}"),
                    format!("Open Project: {name}"),
                    ActionRun::OpenPath(path.clone()),
                )
                .with_subtitle(path.display().to_string())
                .with_icon(ActionIcon::Project)
            })
            .collect()
    }
}

/// `Switch Theme: <name>` entries. Enter dispatches `theme.switch <name>`.
#[derive(Debug, Default, Clone)]
pub struct ThemeProvider {
    themes: Vec<String>,
}

impl ThemeProvider {
    /// Build from a list of available theme names.
    pub fn new(themes: impl IntoIterator<Item = String>) -> Self {
        Self {
            themes: themes.into_iter().collect(),
        }
    }
}

impl ActionProvider for ThemeProvider {
    fn id(&self) -> &str {
        "theme"
    }

    fn search(&self, _query: &str) -> Vec<Action> {
        self.themes
            .iter()
            .map(|name| {
                Action::new(
                    format!("theme:{name}"),
                    format!("Switch Theme: {name}"),
                    ActionRun::Command(
                        CommandId::new(THEME_SWITCH_COMMAND),
                        vec![ActionArg::str(name)],
                    ),
                )
                .with_icon(ActionIcon::Theme)
            })
            .collect()
    }
}

/// `Org Capture: <Template>` (one per configured template) plus a standing
/// `Org: Agenda` entry. The launcher's universal hotkey thus becomes a second
/// entry point to capture, complementing `Ctrl+O`. Capture routes through the
/// `org` IPC service via `org.capture <key>`; the agenda via `org.agenda`.
#[derive(Debug, Default, Clone)]
pub struct OrgProvider {
    /// `(template key, human title)` pairs, as parsed from `org.toml`.
    templates: Vec<(String, String)>,
}

impl OrgProvider {
    /// Build from `(key, title)` template pairs.
    pub fn new(templates: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            templates: templates.into_iter().collect(),
        }
    }
}

impl ActionProvider for OrgProvider {
    fn id(&self) -> &str {
        "org"
    }

    fn search(&self, _query: &str) -> Vec<Action> {
        let mut actions: Vec<Action> = self
            .templates
            .iter()
            .map(|(key, title)| {
                Action::new(
                    format!("org.capture:{key}"),
                    format!("Org Capture: {title}"),
                    ActionRun::Command(
                        CommandId::new(ORG_CAPTURE_COMMAND),
                        vec![ActionArg::str(key)],
                    ),
                )
                .with_icon(ActionIcon::Org)
            })
            .collect();
        actions.push(
            Action::new(
                "org.agenda",
                "Org: Agenda",
                ActionRun::Command(CommandId::new(ORG_AGENDA_COMMAND), Vec::new()),
            )
            .with_icon(ActionIcon::Org),
        );
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculator_emits_result_for_equals_query() {
        let actions = CalculatorProvider.search("=2+2*3");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "8");
        assert!(matches!(
            &actions[0].run,
            ActionRun::Command(id, args)
                if id.as_str() == CLIPBOARD_COPY_COMMAND
                && args == &[ActionArg::str("8")]
        ));
    }

    #[test]
    fn calculator_silent_without_equals_or_on_garbage() {
        assert!(CalculatorProvider.search("2+2").is_empty());
        assert!(CalculatorProvider.search("=not math").is_empty());
        assert!(CalculatorProvider.search("test").is_empty());
    }

    #[test]
    fn url_provider_recognises_links() {
        let actions = UrlProvider.search("https://example.com/path");
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0].run, ActionRun::OpenUrl(u) if u == "https://example.com/path")
        );

        assert!(UrlProvider.search("http://localhost:8080").len() == 1);
    }

    #[test]
    fn url_provider_rejects_non_urls() {
        assert!(UrlProvider.search("https").is_empty());
        assert!(UrlProvider.search("just some text").is_empty());
        assert!(UrlProvider.search("https://has space.com").is_empty());
        assert!(UrlProvider.search("ftp://example.com").is_empty());
    }

    #[test]
    fn recent_projects_emit_open_path_actions() {
        let provider = RecentProjectsProvider::new([
            ("the-app".to_string(), PathBuf::from("/code/app")),
            ("work".to_string(), PathBuf::from("/code/work")),
        ]);
        let actions = provider.search("");
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].title, "Open Project: the-app");
        assert!(matches!(
            &actions[0].run,
            ActionRun::OpenPath(p) if p == &PathBuf::from("/code/app")
        ));
    }

    #[test]
    fn theme_provider_dispatches_switch_command() {
        let provider = ThemeProvider::new(["mocha".to_string(), "latte".to_string()]);
        let actions = provider.search("");
        assert_eq!(actions[0].title, "Switch Theme: mocha");
        assert!(matches!(
            &actions[0].run,
            ActionRun::Command(id, args)
                if id.as_str() == THEME_SWITCH_COMMAND && args == &[ActionArg::str("mocha")]
        ));
    }

    #[test]
    fn org_provider_emits_capture_templates_plus_agenda() {
        let provider = OrgProvider::new([
            ("t".to_string(), "Todo".to_string()),
            ("n".to_string(), "Note".to_string()),
        ]);
        let actions = provider.search("");
        // Two captures + the standing agenda entry.
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].title, "Org Capture: Todo");
        assert!(matches!(
            &actions[0].run,
            ActionRun::Command(id, args)
                if id.as_str() == ORG_CAPTURE_COMMAND && args == &[ActionArg::str("t")]
        ));
        let agenda = actions.last().unwrap();
        assert_eq!(agenda.title, "Org: Agenda");
        assert!(matches!(
            &agenda.run,
            ActionRun::Command(id, args) if id.as_str() == ORG_AGENDA_COMMAND && args.is_empty()
        ));
    }
}
