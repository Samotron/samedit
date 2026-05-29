//! `cockpit-jot` entry point.
//!
//! The full tray app — `tray-icon` menu, `global-hotkey` registration, and the
//! winit popover hosting the org view-models — is the display-bound glue that
//! lands behind the `ui-smoke` feature once it can be smoke-tested. Until then
//! this binary is a headless CLI over the same [`cockpit_jot::JotController`]:
//! it loads the org root and prints today's agenda, proving the wiring works
//! without a window.
//!
//! Usage:
//!   cockpit-jot [--root <dir>] [--config <org.toml>] [agenda|overview]

use std::path::PathBuf;

use anyhow::Result;
use cockpit_jot::app::{HotkeyAction, JotController, Surface};
use cockpit_jot::loader::{
    default_config_path, load_config, load_root, now_stamp, resolve_org_root,
};
use cockpit_ui::AgendaRowKind;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut root_dir: Option<PathBuf> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut command = "agenda".to_string();

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
            "agenda" | "overview" => command = args[i].clone(),
            other => anyhow::bail!("unknown argument: {other}"),
        }
        i += 1;
    }

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
        "overview" => {
            controller.on_hotkey(HotkeyAction::Overview);
            if let Surface::Overview(view) = controller.surface() {
                for row in view.rows() {
                    let indent = "  ".repeat(row.level.saturating_sub(1));
                    println!("{indent}{}", row.label);
                }
            }
        }
        _ => {
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
    }

    eprintln!(
        "\n(note: the tray icon, global hotkeys, and the floating popover are \
         the `ui-smoke` follow-up — this CLI exercises the headless controller.)"
    );
    Ok(())
}
