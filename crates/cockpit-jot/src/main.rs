//! `cockpit-jot` entry point.
//!
//! The full tray app — `tray-icon` menu, `global-hotkey` registration, and the
//! winit popover hosting the org view-models — is the display-bound glue that
//! lands behind the `ui-smoke` feature once it can be smoke-tested. Until then
//! this binary is a headless CLI over the same [`cockpit_jot::JotController`],
//! useful in its own right for scripts and editor keybindings:
//!
//!   cockpit-jot [--root <dir>] [--config <org.toml>] agenda
//!   cockpit-jot [--root <dir>] [--config <org.toml>] overview
//!   cockpit-jot [--root <dir>] [--config <org.toml>] capture <key> [title...]
//!
//! `agenda` / `overview` print the corresponding view; `capture` runs a
//! configured template to completion and writes the entry to disk — the same
//! `WriteFile` intent the popover would carry out.

use std::fs;
use std::path::PathBuf;

use anyhow::{Result, bail};
use cockpit_jot::app::{HotkeyAction, JotController, JotIntent, Surface};
use cockpit_jot::loader::{
    default_config_path, load_config, load_root, now_stamp, resolve_org_root,
};
use cockpit_ui::AgendaRowKind;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root_dir: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                root_dir = args.get(i).map(PathBuf::from);
            }
            "--config" => {
                i += 1;
                config_path = args.get(i).map(PathBuf::from);
            }
            flag if flag.starts_with("--") => bail!("unknown flag: {flag}"),
            _ => positional.push(args[i].clone()),
        }
        i += 1;
    }

    let command = positional.first().cloned().unwrap_or_else(|| "agenda".into());

    // Load capture templates + workflow from `org.toml` (default
    // `~/.config/cockpit/org.toml`); a missing file falls back to defaults.
    let config = match config_path.or_else(default_config_path) {
        Some(path) => load_config(&path)?,
        None => cockpit_org::OrgConfig::default(),
    };

    // Root precedence: `--root` > config `root` > `~/org`.
    let root_dir = resolve_org_root(root_dir, &config);
    let root = load_root(&root_dir, &config)?;
    let mut controller = JotController::new(config, root, now_stamp());

    match command.as_str() {
        "capture" => run_capture(&mut controller, &positional[1..])?,
        "overview" => print_overview(&mut controller),
        "agenda" => print_agenda(&mut controller),
        other => bail!("unknown command: {other} (expected agenda | overview | capture)"),
    }

    Ok(())
}

/// `capture <key> [title...]`: pick the template, drop the joined title into
/// the `%?` slot, commit, and execute the resulting `WriteFile` intent.
fn run_capture(controller: &mut JotController, rest: &[String]) -> Result<()> {
    controller.on_hotkey(HotkeyAction::Capture);

    // The keys available from the loaded config, for help / error messages.
    let available = available_templates(controller);

    let Some(key) = rest.first() else {
        bail!("capture needs a template key.{}", available_hint(&available));
    };

    if !controller.capture_pick(key) {
        bail!("no capture template with key '{key}'.{}", available_hint(&available));
    }

    let title = rest[1..].join(" ");
    if !title.is_empty() {
        controller.capture_insert_str(&title);
    }

    let mut wrote = false;
    for intent in controller.capture_commit() {
        if let JotIntent::WriteFile { path, source } = intent {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, &source)?;
            println!("Captured to {}", path.display());
            wrote = true;
        }
    }
    if !wrote {
        bail!("capture produced no entry (template '{key}' may be empty)");
    }
    Ok(())
}

fn print_overview(controller: &mut JotController) {
    controller.on_hotkey(HotkeyAction::Overview);
    if let Surface::Overview(view) = controller.surface() {
        for row in view.rows() {
            let indent = "  ".repeat(row.level.saturating_sub(1));
            println!("{indent}{}", row.label);
        }
    }
}

fn print_agenda(controller: &mut JotController) {
    controller.on_hotkey(HotkeyAction::Agenda);
    let today = controller.today();
    if let Surface::Agenda(view) = controller.surface() {
        println!(
            "Agenda — {} ({:04}-{:02}-{:02}):",
            view.mode().label(),
            today.year,
            today.month,
            today.day
        );
        for row in view.rows() {
            let marker = if row.overdue { "! " } else { "  " };
            let bullet = match row.kind {
                AgendaRowKind::Item => marker,
                _ => "",
            };
            println!("{bullet}{}", row.label);
        }
    }
}

/// `(key, name)` of every configured template, read off the open capture
/// surface.
fn available_templates(controller: &JotController) -> Vec<(String, String)> {
    if let Surface::Capture(view) = controller.surface() {
        view.template_rows()
            .into_iter()
            .map(|row| (row.key, row.name))
            .collect()
    } else {
        Vec::new()
    }
}

fn available_hint(available: &[(String, String)]) -> String {
    if available.is_empty() {
        " No capture templates are configured in org.toml.".to_string()
    } else {
        let list = available
            .iter()
            .map(|(key, name)| format!("{key} ({name})"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" Available: {list}.")
    }
}
