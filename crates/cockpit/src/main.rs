//! `cockpit` — a fast, native, multi-platform coding cockpit.
//!
//! The binary wires the headless cores to the windowing harness. `--fixture`
//! (spec §18.12, M1.21) and `--project` open a project in a real window; pass
//! `--print` for the headless project-detection dump used by CI and humans
//! without a display server.

mod app;
mod hydration;
mod launcher;
mod mux_layout;
mod splash;
mod startup;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use cockpit_project::{ProjectDetection, RecentProjects, detect_project, recent_projects_path};
use cockpit_testkit::{BUILTIN_FIXTURES, fixture_path};

use crate::app::AppShell;
use crate::launcher::LauncherModel;

#[derive(Debug, Parser)]
#[command(name = "cockpit", version, about = "Coding Cockpit")]
struct Cli {
    /// Open a bundled fixture project by name (spec §18.12).
    #[arg(long, value_name = "NAME", conflicts_with = "project")]
    fixture: Option<String>,

    /// Open a project at this path.
    #[arg(long, value_name = "PATH")]
    project: Option<PathBuf>,

    /// Print project detection to stdout instead of opening a window.
    #[arg(long)]
    print: bool,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("COCKPIT_LOG")
                .or_else(|_| tracing_subscriber::EnvFilter::try_from_default_env())
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("cockpit: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let path = if let Some(name) = cli.fixture {
        if !BUILTIN_FIXTURES.contains(&name.as_str()) {
            anyhow::bail!(
                "unknown fixture `{name}`. Bundled fixtures: {}",
                BUILTIN_FIXTURES.join(", ")
            );
        }
        Some(fixture_path(&name))
    } else {
        cli.project
    };

    // `--print` short-circuits without opening a window. It requires a
    // path — there is no headless launcher.
    if cli.print {
        let path = path.context("--print requires --project or --fixture")?;
        let detection = startup::time_phase("startup.detect", || {
            detect_project(&path).with_context(|| format!("detect project at `{}`", path.display()))
        })?;
        record_recent_project(&detection);
        print_detection(&detection);
        return Ok(());
    }

    // Build the single [`AppShell`] the harness will drive. The shell
    // owns the transition from launcher → hydrating → live, so M7.1's
    // hard rule ("run_app at most once per process") holds even when
    // the user picks a project from the launcher.
    let (title, shell) = match path {
        Some(path) => {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
                .to_string();
            tracing::info!(project = %name, "opening window with splash");
            (
                format!("Coding Cockpit — {name}"),
                AppShell::hydrating(path),
            )
        }
        None => {
            tracing::info!("opening project launcher");
            (
                "Coding Cockpit".to_string(),
                AppShell::launcher(LauncherModel::new(load_recent_projects())),
            )
        }
    };

    cockpit_render::run_app(title, shell).context("windowing harness failed")?;
    Ok(())
}

/// Load the persisted recent-projects list as the launcher view-model
/// expects it. Empty (no panic) on any IO failure — the launcher is
/// still usable, it just shows the "no recent projects" state.
fn load_recent_projects() -> Vec<cockpit_ui::launcher::RecentProject> {
    recent_projects_path()
        .ok()
        .and_then(|path| RecentProjects::load(path).ok())
        .map(|r| {
            r.projects
                .into_iter()
                .map(|p| cockpit_ui::launcher::RecentProject::new(p.display_name, p.root_path))
                .collect()
        })
        .unwrap_or_default()
}

/// Add a project to the launcher's recent-projects registry. Best-effort: a
/// cache failure must never stop the project from opening. Used by the
/// `--print` path; the hydration driver records recents itself on the
/// normal path.
fn record_recent_project(detection: &ProjectDetection) {
    let Ok(path) = recent_projects_path() else {
        return;
    };
    let mut recents = RecentProjects::load(&path).unwrap_or_default();
    recents.record(&detection.root_path, &detection.display_name);
    if let Err(err) = recents.store(&path) {
        tracing::warn!(error = %err, "failed to store recent projects");
    }
}

fn print_detection(detection: &ProjectDetection) {
    println!("Project:   {}", detection.display_name);
    println!("Root:      {}", detection.root_path.display());
    match detection.strongest_signal {
        Some(kind) => println!("Strongest: {kind:?}"),
        None => println!("Strongest: (none detected)"),
    }
    println!("Signals:");
    if detection.signals.is_empty() {
        println!("  (none)");
    } else {
        for signal in &detection.signals {
            println!("  - {:?}: {}", signal.kind, signal.path.display());
        }
    }

    let mise = &detection.mise;
    if !mise.tools.is_empty() || !mise.tasks.is_empty() {
        println!("Mise:");
        if !mise.tools.is_empty() {
            println!("  Tools:");
            for tool in &mise.tools {
                println!("    - {} = {}", tool.name, tool.version);
            }
        }
        if !mise.tasks.is_empty() {
            println!("  Tasks:");
            for task in &mise.tasks {
                let desc = task
                    .description
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" — {d}"))
                    .unwrap_or_default();
                println!("    - {}{desc}", task.name);
            }
        }
        if let Some(cockpit) = &mise.metadata {
            println!("  Cockpit metadata:");
            if let Some(name) = cockpit.name.as_deref() {
                println!("    name = {name}");
            }
            if let Some(default_task) = cockpit.default_task.as_deref() {
                println!("    default_task = {default_task}");
            }
            if let Some(terminal_workspace) = cockpit.terminal_workspace.as_deref() {
                println!("    terminal_workspace = {terminal_workspace}");
            }
            if let Some(layout) = cockpit.cockpit_layout.as_deref() {
                println!("    cockpit_layout = {}", layout.display());
            }
            if let Some(layout) = cockpit.zellij_layout.as_deref() {
                println!(
                    "    zellij_layout = {} (deprecated, ignored)",
                    layout.display()
                );
            }
        }
    }
}
