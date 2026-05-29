//! The mise-tasks provider (M13.2).
//!
//! Every task across the registered mise projects becomes a launcher entry;
//! hitting Enter spawns `mise run <task>` in that project's root. Project
//! discovery is **explicit** — the launcher reads a list of project paths
//! (from `launcher.toml`'s `[mise.projects]`), never crawls the filesystem.
//!
//! Parsing reuses `cockpit_project::detect_mise_project_with`, so the task
//! grammar stays identical to the in-cockpit project layer. The provider is
//! built from already-parsed data via [`MiseTasksProvider::from_projects`],
//! which takes an injected [`FileSystem`](cockpit_project::env::FileSystem) —
//! no real disk, fully testable.

use std::path::{Path, PathBuf};

use cockpit_project::env::{FileSystem, ProcessSpec};
use cockpit_project::{CockpitMetadata, detect_mise_project_with, env};

use crate::action::{Action, ActionIcon, ActionRun};
use crate::provider::ActionProvider;

/// One discovered task: which project it belongs to and how to run it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskEntry {
    project_name: String,
    project_root: PathBuf,
    task_name: String,
    description: Option<String>,
}

/// Provider over the tasks of a fixed set of mise projects.
#[derive(Debug, Default, Clone)]
pub struct MiseTasksProvider {
    entries: Vec<TaskEntry>,
}

impl MiseTasksProvider {
    /// Build a provider by parsing each project root's `mise.toml` through the
    /// injected filesystem. Roots without a parseable mise config contribute
    /// no tasks (they are skipped, not an error — a stale `launcher.toml`
    /// entry shouldn't break the whole launcher).
    pub fn from_projects<I, P>(fs: &dyn FileSystem, roots: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        // The mise-availability probe is irrelevant to task discovery (we only
        // read the config), so a never-available process runner keeps parsing
        // pure — no spawn, no `$PATH` lookup.
        let process = NoProcess;
        let mut entries = Vec::new();
        for root in roots {
            let root = root.as_ref();
            let Ok(project) = detect_mise_project_with(root, fs, &process) else {
                continue;
            };
            if !project.detected {
                continue;
            }
            let project_name = project_name(root, project.metadata.as_ref());
            for task in project.tasks {
                entries.push(TaskEntry {
                    project_name: project_name.clone(),
                    project_root: root.to_path_buf(),
                    task_name: task.name,
                    description: task.description,
                });
            }
        }
        entries.sort_by(|a, b| {
            a.project_name
                .cmp(&b.project_name)
                .then_with(|| a.task_name.cmp(&b.task_name))
        });
        Self { entries }
    }

    /// Number of discovered tasks.
    pub fn task_count(&self) -> usize {
        self.entries.len()
    }
}

impl ActionProvider for MiseTasksProvider {
    fn id(&self) -> &str {
        "mise"
    }

    fn search(&self, _query: &str) -> Vec<Action> {
        // The launcher fuzzy-filters; the provider just emits every task.
        self.entries
            .iter()
            .map(|entry| {
                let title = format!("{}: {}", entry.project_name, entry.task_name);
                let spec = ProcessSpec::new("mise")
                    .arg("run")
                    .arg(&entry.task_name)
                    .current_dir(&entry.project_root);
                let subtitle = entry
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("mise run {}", entry.task_name));
                Action::new(
                    format!("mise:{}:{}", entry.project_name, entry.task_name),
                    title,
                    ActionRun::Process(spec),
                )
                .with_subtitle(subtitle)
                .with_icon(ActionIcon::Task)
            })
            .collect()
    }
}

/// Project label: the `[metadata.cockpit] name`, else the root's final path
/// component, else the whole path.
fn project_name(root: &Path, metadata: Option<&CockpitMetadata>) -> String {
    if let Some(name) = metadata.and_then(|m| m.name.clone()) {
        return name;
    }
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

/// A `ProcessRunner` that never spawns — task discovery only reads files, so
/// the mise-availability probe must not touch the real environment.
struct NoProcess;

impl env::ProcessRunner for NoProcess {
    fn run(&self, _spec: &ProcessSpec) -> std::io::Result<env::ProcessOutput> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "process spawning disabled during launcher task discovery",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::env::FakeFileSystem;

    fn fs() -> FakeFileSystem {
        let fs = FakeFileSystem::new();
        fs.insert_file(
            "/code/work/mise.toml",
            "[tasks.test]\nrun = \"cargo test\"\ndescription = \"run tests\"\n\
             [tasks.build]\nrun = \"cargo build\"\n",
        );
        fs.insert_file(
            "/code/app/mise.toml",
            "[metadata.cockpit]\nname = \"the-app\"\n[tasks.test-ui]\nrun = \"npm test\"\n",
        );
        fs
    }

    #[test]
    fn discovers_tasks_across_projects() {
        let provider = MiseTasksProvider::from_projects(&fs(), ["/code/work", "/code/app"]);
        assert_eq!(provider.task_count(), 3);

        let actions = provider.search("");
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
        // Sorted by project name (metadata name "the-app" < "work") then task.
        assert_eq!(
            titles,
            vec!["the-app: test-ui", "work: build", "work: test"]
        );
    }

    #[test]
    fn task_action_runs_mise_in_project_root() {
        let provider = MiseTasksProvider::from_projects(&fs(), ["/code/work"]);
        let action = provider
            .search("")
            .into_iter()
            .find(|a| a.title == "work: test")
            .expect("test task present");
        match action.run {
            ActionRun::Process(spec) => {
                assert_eq!(spec.program, "mise");
                assert_eq!(spec.args, ["run", "test"]);
                assert_eq!(spec.current_dir.as_deref(), Some(Path::new("/code/work")));
            }
            other => panic!("expected process run, got {other:?}"),
        }
    }

    #[test]
    fn description_falls_back_to_run_command() {
        let provider = MiseTasksProvider::from_projects(&fs(), ["/code/work"]);
        let build = provider
            .search("")
            .into_iter()
            .find(|a| a.title == "work: build")
            .unwrap();
        assert_eq!(build.subtitle.as_deref(), Some("mise run build"));
    }

    #[test]
    fn unknown_project_is_skipped_not_fatal() {
        let provider =
            MiseTasksProvider::from_projects(&fs(), ["/code/work", "/code/does-not-exist"]);
        assert_eq!(provider.task_count(), 2);
    }
}
