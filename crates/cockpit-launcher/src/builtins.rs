//! Built-in providers that need no IPC or external state (M13.4, headless
//! subset): the calculator and the URL opener.
//!
//! The other built-ins from the plan — `Open Project`, `Switch Theme`,
//! `Org Capture`, `Org Agenda` — dispatch over the `cockpit` / `org` IPC
//! services, so they land with the `cockpit-quick` binary (M13.5/M13.7) where
//! a live IPC client exists. These two are self-contained: they turn the
//! query itself into an action.

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
}
