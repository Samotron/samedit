//! `cockpit` — a fast, native, multi-platform coding cockpit.
//!
//! The binary wires the headless cores to the windowing harness. `--fixture`
//! (spec §18.12, M1.21) and `--project` open a project in a real window; pass
//! `--print` for the headless project-detection dump used by CI and humans
//! without a display server.

mod app;
mod hydration;
mod launcher;
mod splash;
mod startup;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use cockpit_project::{ProjectDetection, RecentProjects, detect_project, recent_projects_path};
use cockpit_testkit::{BUILTIN_FIXTURES, fixture_path};

use crate::app::AppShell;
use crate::launcher::{LauncherModel, LauncherResult};

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
    if let Some(name) = cli.fixture {
        if !BUILTIN_FIXTURES.contains(&name.as_str()) {
            anyhow::bail!(
                "unknown fixture `{name}`. Bundled fixtures: {}",
                BUILTIN_FIXTURES.join(", ")
            );
        }
        run_project_or_launcher(Some(fixture_path(&name)), cli.print)
    } else if let Some(path) = cli.project {
        run_project_or_launcher(Some(path), cli.print)
    } else {
        run_project_or_launcher(None, cli.print)
    }
}

/// Start either a project workspace or the project launcher. If a launcher
/// selection results in a project, this function tail-calls itself to open
/// that project.
///
/// The normal path opens the window with the splash painted on frame 1
/// and defers detection / tree load / model build / config / git / cache
/// to the per-frame [`crate::hydration::HydrationDriver`] (v0.6 M6.2).
/// `--print` stays fully synchronous because it never opens a window.
fn run_project_or_launcher(path: Option<PathBuf>, print: bool) -> Result<()> {
    let Some(path) = path else {
        return run_launcher();
    };

    if print {
        // Headless path: do the work inline and dump detection to
        // stdout. No window, no splash.
        let detection = startup::time_phase("startup.detect", || {
            detect_project(&path).with_context(|| format!("detect project at `{}`", path.display()))
        })?;
        record_recent_project(&detection);
        print_detection(&detection);
        return Ok(());
    }

    let initial_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    let title = format!("Coding Cockpit — {initial_name}");
    tracing::info!(project = initial_name, "opening window with splash");

    cockpit_render::run_app(title, AppShell::hydrating(path))
        .context("windowing harness failed")?;
    Ok(())
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

/// Run the GUI project launcher (spec §6, M1.13).
fn run_launcher() -> Result<()> {
    let recents = recent_projects_path()
        .ok()
        .and_then(|path| RecentProjects::load(path).ok())
        .map(|r| {
            r.projects
                .into_iter()
                .map(|p| cockpit_ui::launcher::RecentProject::new(p.display_name, p.root_path))
                .collect()
        })
        .unwrap_or_default();

    let mut model = LauncherModel::new(recents);
    cockpit_render::run_app("Coding Cockpit", &mut model).context("launcher harness failed")?;

    match model.result() {
        Some(LauncherResult::OpenProject(path)) => run_project_or_launcher(Some(path), false),
        Some(LauncherResult::Exit) | None => Ok(()),
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
            if let Some(layout) = cockpit.zellij_layout.as_deref() {
                println!("    zellij_layout = {}", layout.display());
            }
        }
    }
}
