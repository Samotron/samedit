//! `cockpit-quick` binary entry point.
//!
//! Until the winit popover shell lands (behind `--features ui-smoke`), this is
//! a headless CLI over the same [`QuickController`] the popover will drive:
//!
//! ```text
//! cockpit-quick search [query...]   # print the ranked launcher rows
//! cockpit-quick run    [query...]   # print the intent for the top row
//! cockpit-quick providers           # list the enabled providers
//! ```
//!
//! Config is read from `~/.config/cockpit/launcher.toml` (defaults apply when
//! absent). Runtime inputs that arrive over IPC in the real app
//! (recent projects, themes, org templates, Lua actions) are empty in the
//! CLI — the mise and built-in calculator/URL providers still work fully,
//! which is enough to exercise ranking and dispatch from scripts.

use std::process::ExitCode;

use cockpit_config::{LauncherConfig, launcher_config_path};
use cockpit_project::env::StdFileSystem;
use cockpit_quick::{ProviderInputs, QuickController, QuickEvent, QuickIntent, build_launcher};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((cmd, rest)) => (cmd.as_str(), rest),
        None => {
            eprintln!("usage: cockpit-quick <search|run|providers> [query...]");
            return ExitCode::FAILURE;
        }
    };

    let config = load_config();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let inputs = ProviderInputs::default();
    let fs = StdFileSystem;
    let launcher = build_launcher(&config, &inputs, home.as_deref(), &fs);

    match command {
        "providers" => {
            for id in launcher.provider_ids() {
                println!("{id}");
            }
            ExitCode::SUCCESS
        }
        "search" => {
            let query = rest.join(" ");
            let mut controller = QuickController::new(launcher);
            controller.handle(QuickEvent::SetQuery(query));
            if controller.results().is_empty() {
                println!("(no matches)");
            }
            for (i, ranked) in controller.results().iter().enumerate() {
                let marker = if i == controller.selection() {
                    '>'
                } else {
                    ' '
                };
                let subtitle = ranked
                    .action
                    .subtitle
                    .as_deref()
                    .map(|s| format!("  — {s}"))
                    .unwrap_or_default();
                println!(
                    "{marker} [{}] {}{subtitle}",
                    ranked.provider, ranked.action.title
                );
            }
            ExitCode::SUCCESS
        }
        "run" => {
            let query = rest.join(" ");
            let mut controller = QuickController::new(launcher);
            controller.handle(QuickEvent::SetQuery(query));
            match controller.handle(QuickEvent::Submit) {
                Some(intent) => {
                    println!("{}", describe_intent(&intent));
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("no action to run");
                    ExitCode::FAILURE
                }
            }
        }
        other => {
            eprintln!("unknown command `{other}` (expected search|run|providers)");
            ExitCode::FAILURE
        }
    }
}

/// Load `launcher.toml`, falling back to defaults if it (or its path) is
/// absent. A malformed file is surfaced as a warning, then defaults apply, so
/// the CLI never hard-fails on a typo.
fn load_config() -> LauncherConfig {
    let Some(path) = launcher_config_path() else {
        return LauncherConfig::default();
    };
    match LauncherConfig::load_optional(&path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("warning: {} — using defaults: {err}", path.display());
            LauncherConfig::default()
        }
    }
}

/// One-line, human-readable description of a lowered intent (the CLI's stand-in
/// for the real effect the popover shell performs).
fn describe_intent(intent: &QuickIntent) -> String {
    match intent {
        QuickIntent::CopyToClipboard(text) => format!("copy to clipboard: {text}"),
        QuickIntent::OpenUrl(url) => format!("open url: {url}"),
        QuickIntent::OpenPath(path) => format!("open project: {}", path.display()),
        QuickIntent::DispatchCommand { command, args } => {
            format!("dispatch command: {command} {}", args.join(" "))
        }
        QuickIntent::RunLua(handle) => {
            format!("run lua action: {}:{}", handle.extension, handle.id)
        }
        QuickIntent::RunProcess(spec) => {
            let args: Vec<String> = spec
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            format!(
                "run process: {} {}",
                spec.program.to_string_lossy(),
                args.join(" ")
            )
        }
        QuickIntent::Dismiss => "dismiss".to_string(),
    }
}
