//! `cockpit` — a fast, native, multi-platform coding cockpit.
//!
//! The binary wires the headless cores to the windowing harness. `--fixture`
//! (spec §18.12, M1.21) and `--project` open a project in a real window; pass
//! `--print` for the headless project-detection dump used by CI and humans
//! without a display server.

mod app;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use cockpit_project::{FileTree, ProjectDetection, detect_project};
use cockpit_testkit::{BUILTIN_FIXTURES, fixture_path};

use crate::app::{AppModel, AppShell};

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
        run_fixture(&name, cli.print)
    } else if let Some(path) = cli.project {
        run_project(&path, cli.print)
    } else {
        println!(
            "cockpit {} — coding cockpit shell.",
            env!("CARGO_PKG_VERSION")
        );
        println!("Pass --fixture <name> or --project <path> to open a project.");
        println!("Add --print for a headless project-detection dump.");
        println!("Bundled fixtures: {}", BUILTIN_FIXTURES.join(", "));
        Ok(())
    }
}

fn run_fixture(name: &str, print: bool) -> Result<()> {
    if !BUILTIN_FIXTURES.contains(&name) {
        anyhow::bail!(
            "unknown fixture `{name}`. Bundled fixtures: {}",
            BUILTIN_FIXTURES.join(", ")
        );
    }
    let path = fixture_path(name);
    tracing::info!(fixture = name, path = %path.display(), "loading fixture");
    run_project(&path, print)
}

fn run_project(path: &std::path::Path, print: bool) -> Result<()> {
    let detection =
        detect_project(path).with_context(|| format!("detect project at `{}`", path.display()))?;

    if print {
        print_detection(&detection);
        return Ok(());
    }

    let tree =
        FileTree::load(path).with_context(|| format!("load file tree at `{}`", path.display()))?;
    let mut model = AppModel::new(detection, tree).map_err(|err| anyhow!(err))?;
    model.restore_cached_state();
    let title = format!("Coding Cockpit — {}", model.project_name());
    tracing::info!(project = model.project_name(), "opening window");

    cockpit_render::run_app(title, AppShell::new(model)).context("windowing harness failed")?;
    Ok(())
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
