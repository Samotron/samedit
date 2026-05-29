//! The headless launcher controller — the backend-free brain of
//! `cockpit-quick` (M13.5).
//!
//! Mirrors `cockpit-jot`'s `JotController`: it owns the live [`Launcher`] and
//! the current query/results/selection, and maps **events** (query edits,
//! navigation, submit) onto **intents** the shell carries out. The winit
//! popover and the headless CLI both drive this same controller, so all the
//! interesting logic is unit-tested without a window.

use std::path::PathBuf;

use cockpit_launcher::{
    Action, ActionArg, ActionRun, CLIPBOARD_COPY_COMMAND, Launcher, LuaActionHandle, RankedAction,
};
use cockpit_project::env::ProcessSpec;

/// An input event from the shell (popover keys) or the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickEvent {
    /// Replace the query text and re-rank.
    SetQuery(String),
    /// Move the selection up one row.
    MoveUp,
    /// Move the selection down one row.
    MoveDown,
    /// Activate the selected row.
    Submit,
    /// Dismiss the launcher.
    Dismiss,
}

/// A lowered intent for the shell/binary to perform. Every launcher
/// [`ActionRun`] resolves to one of these; the binary turns it into a real
/// effect (clipboard write, browser open, IPC send to the cockpit, process
/// spawn, Lua invocation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuickIntent {
    /// Copy text to the clipboard (the calculator result).
    CopyToClipboard(String),
    /// Open a URL in the browser.
    OpenUrl(String),
    /// Open/focus a project or file in the cockpit — lowered onto a
    /// `cockpit`-service `OpenProject` message by the binary.
    OpenPath(PathBuf),
    /// Dispatch a command in the cockpit — lowered onto a `cockpit`-service
    /// `DispatchCommand` message (or routed to the `org` service for
    /// `org.*`).
    DispatchCommand { command: String, args: Vec<String> },
    /// Run a registered Lua launcher action in its owning VM.
    RunLua(LuaActionHandle),
    /// Spawn a process (a mise task).
    RunProcess(ProcessSpec),
    /// Close the launcher without doing anything else.
    Dismiss,
}

/// The launcher controller. Holds the matcher and the current view state.
pub struct QuickController {
    launcher: Launcher,
    query: String,
    results: Vec<RankedAction>,
    selection: usize,
}

impl QuickController {
    /// Build a controller over an assembled [`Launcher`]. Starts on the empty
    /// query (the "favourites" listing).
    pub fn new(launcher: Launcher) -> Self {
        let mut controller = Self {
            launcher,
            query: String::new(),
            results: Vec::new(),
            selection: 0,
        };
        controller.recompute();
        controller
    }

    /// Current query text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Current ranked results.
    pub fn results(&self) -> &[RankedAction] {
        &self.results
    }

    /// Index of the highlighted row.
    pub fn selection(&self) -> usize {
        self.selection
    }

    /// The highlighted action, if any.
    pub fn selected(&self) -> Option<&Action> {
        self.results.get(self.selection).map(|r| &r.action)
    }

    /// Apply an event, returning an intent when one fires (on submit/dismiss).
    pub fn handle(&mut self, event: QuickEvent) -> Option<QuickIntent> {
        match event {
            QuickEvent::SetQuery(query) => {
                self.query = query;
                self.recompute();
                None
            }
            QuickEvent::MoveUp => {
                self.selection = self.selection.saturating_sub(1);
                None
            }
            QuickEvent::MoveDown => {
                if self.selection + 1 < self.results.len() {
                    self.selection += 1;
                }
                None
            }
            QuickEvent::Submit => self.selected().map(lower_action),
            QuickEvent::Dismiss => Some(QuickIntent::Dismiss),
        }
    }

    fn recompute(&mut self) {
        self.results = self.launcher.search(&self.query);
        if self.selection >= self.results.len() {
            self.selection = 0;
        }
    }
}

/// Lower a selected action's [`ActionRun`] onto a [`QuickIntent`].
fn lower_action(action: &Action) -> QuickIntent {
    match &action.run {
        ActionRun::Command(id, args) if id.as_str() == CLIPBOARD_COPY_COMMAND => {
            // The calculator's copy command — the one Command lowered to a
            // local effect rather than an IPC dispatch.
            QuickIntent::CopyToClipboard(first_arg(args))
        }
        ActionRun::Command(id, args) => QuickIntent::DispatchCommand {
            command: id.as_str().to_string(),
            args: args.iter().map(arg_to_string).collect(),
        },
        ActionRun::OpenUrl(url) => QuickIntent::OpenUrl(url.clone()),
        ActionRun::OpenPath(path) => QuickIntent::OpenPath(path.clone()),
        ActionRun::Process(spec) => QuickIntent::RunProcess(spec.clone()),
        ActionRun::Lua(handle) => QuickIntent::RunLua(handle.clone()),
    }
}

fn arg_to_string(arg: &ActionArg) -> String {
    match arg {
        ActionArg::Str(s) => s.clone(),
        ActionArg::Path(p) => p.display().to_string(),
    }
}

fn first_arg(args: &[ActionArg]) -> String {
    args.first().map(arg_to_string).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_launcher::{CalculatorProvider, ThemeProvider, UrlProvider};

    fn controller() -> QuickController {
        let mut launcher = Launcher::new();
        launcher
            .register(Box::new(CalculatorProvider))
            .register(Box::new(UrlProvider))
            .register(Box::new(ThemeProvider::new([
                "mocha".to_string(),
                "latte".to_string(),
            ])));
        QuickController::new(launcher)
    }

    #[test]
    fn empty_query_lists_favourites() {
        let c = controller();
        // The theme provider's two entries show on the empty query.
        assert_eq!(c.results().len(), 2);
    }

    #[test]
    fn calculator_submit_copies_to_clipboard() {
        let mut c = controller();
        c.handle(QuickEvent::SetQuery("=2+2*3".to_string()));
        let intent = c.handle(QuickEvent::Submit).unwrap();
        assert_eq!(intent, QuickIntent::CopyToClipboard("8".to_string()));
    }

    #[test]
    fn url_submit_opens_browser() {
        let mut c = controller();
        c.handle(QuickEvent::SetQuery("https://example.com".to_string()));
        let intent = c.handle(QuickEvent::Submit).unwrap();
        assert_eq!(
            intent,
            QuickIntent::OpenUrl("https://example.com".to_string())
        );
    }

    #[test]
    fn theme_submit_lowers_to_dispatch_command() {
        let mut c = controller();
        c.handle(QuickEvent::SetQuery("mocha".to_string()));
        let intent = c.handle(QuickEvent::Submit).unwrap();
        assert_eq!(
            intent,
            QuickIntent::DispatchCommand {
                command: "theme.switch".to_string(),
                args: vec!["mocha".to_string()],
            }
        );
    }

    #[test]
    fn navigation_saturates_and_dismiss_fires() {
        let mut c = controller();
        c.handle(QuickEvent::MoveUp); // already at top
        assert_eq!(c.selection(), 0);
        c.handle(QuickEvent::MoveDown);
        assert_eq!(c.selection(), 1);
        for _ in 0..10 {
            c.handle(QuickEvent::MoveDown);
        }
        assert_eq!(c.selection(), c.results().len() - 1);
        assert_eq!(c.handle(QuickEvent::Dismiss), Some(QuickIntent::Dismiss));
    }

    #[test]
    fn submit_with_no_results_is_noop() {
        let mut c = controller();
        c.handle(QuickEvent::SetQuery("zzz-no-match-zzz".to_string()));
        assert!(c.handle(QuickEvent::Submit).is_none());
    }
}
