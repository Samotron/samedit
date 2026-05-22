//! `cockpit-project` — projects, `mise` integration, and the file tree.
//!
//! Project detection (spec §6), the `mise` environment provider (spec §8),
//! the per-project state cache (spec §7), and the lazy file tree (spec §13).

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const PROJECT_SIGNALS: &[(&str, ProjectSignalKind)] = &[
    ("mise.toml", ProjectSignalKind::Mise),
    (".mise.toml", ProjectSignalKind::Mise),
    (".git", ProjectSignalKind::Git),
    ("Cargo.toml", ProjectSignalKind::Rust),
    ("build.zig", ProjectSignalKind::Zig),
    ("pyproject.toml", ProjectSignalKind::Python),
    ("package.json", ProjectSignalKind::Node),
    ("go.mod", ProjectSignalKind::Go),
    ("pom.xml", ProjectSignalKind::Java),
    ("build.gradle", ProjectSignalKind::Java),
];

/// A detected project signal file or directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectSignalKind {
    Mise,
    Git,
    Rust,
    Zig,
    Python,
    Node,
    Go,
    Java,
}

/// One signal that contributed to project detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSignal {
    pub kind: ProjectSignalKind,
    pub path: PathBuf,
}

/// Result of detecting a project root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDetection {
    pub root_path: PathBuf,
    pub display_name: String,
    pub signals: Vec<ProjectSignal>,
    pub strongest_signal: Option<ProjectSignalKind>,
    pub mise: MiseProject,
}

impl ProjectDetection {
    /// True when at least one known project signal was found.
    pub fn detected(&self) -> bool {
        !self.signals.is_empty()
    }
}

/// Detect known project signals and parse optional mise configuration.
pub fn detect_project(root_path: impl AsRef<Path>) -> Result<ProjectDetection, ProjectError> {
    let root_path = root_path.as_ref().to_path_buf();
    let display_name = root_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string();

    let signals = detect_signals(&root_path);
    let strongest_signal = signals.first().map(|signal| signal.kind);
    let mise = detect_mise_project(&root_path)?;

    Ok(ProjectDetection {
        root_path,
        display_name,
        signals,
        strongest_signal,
        mise,
    })
}

fn detect_signals(root_path: &Path) -> Vec<ProjectSignal> {
    PROJECT_SIGNALS
        .iter()
        .filter_map(|(name, kind)| {
            let path = root_path.join(name);
            path.exists().then_some(ProjectSignal { kind: *kind, path })
        })
        .collect()
}

/// Parsed mise project information.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MiseProject {
    pub detected: bool,
    pub available: bool,
    pub config_path: Option<PathBuf>,
    pub tools: Vec<Tool>,
    pub tasks: Vec<Task>,
    pub env: Vec<EnvVar>,
    pub metadata: Option<CockpitMetadata>,
}

/// A configured mise tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub version: String,
}

/// A configured mise task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub name: String,
    pub description: Option<String>,
    pub run: Option<String>,
}

/// A configured mise environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// Optional `[metadata.cockpit]` block from `mise.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CockpitMetadata {
    pub name: Option<String>,
    pub default_task: Option<String>,
    pub terminal_workspace: Option<String>,
    pub zellij_layout: Option<PathBuf>,
}

/// Parse `mise.toml` / `.mise.toml` from a project root.
pub fn detect_mise_project(root_path: impl AsRef<Path>) -> Result<MiseProject, ProjectError> {
    let root_path = root_path.as_ref();
    let Some(config_path) = mise_config_path(root_path) else {
        return Ok(MiseProject::default());
    };

    let input = fs::read_to_string(&config_path).map_err(ProjectError::Read)?;
    let mut project = parse_mise_toml(&input)?;
    project.detected = true;
    project.available = mise_available();
    project.config_path = Some(config_path);
    Ok(project)
}

fn mise_config_path(root_path: &Path) -> Option<PathBuf> {
    ["mise.toml", ".mise.toml"]
        .into_iter()
        .map(|name| root_path.join(name))
        .find(|path| path.exists())
}

fn mise_available() -> bool {
    std::process::Command::new("mise")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Parse a mise TOML document.
pub fn parse_mise_toml(input: &str) -> Result<MiseProject, ProjectError> {
    let raw: RawMiseConfig = toml::from_str(input).map_err(ProjectError::Parse)?;

    Ok(MiseProject {
        detected: true,
        available: false,
        config_path: None,
        tools: raw
            .tools
            .into_iter()
            .map(|(name, version)| Tool {
                name,
                version: value_to_string(version),
            })
            .collect(),
        tasks: raw
            .tasks
            .into_iter()
            .map(|(name, task)| Task {
                name,
                description: task.description,
                run: task.run.map(value_to_string),
            })
            .collect(),
        env: raw
            .env
            .into_iter()
            .map(|(name, value)| EnvVar {
                name,
                value: value_to_string(value),
            })
            .collect(),
        metadata: raw.metadata.and_then(|metadata| metadata.cockpit),
    })
}

fn value_to_string(value: toml::Value) -> String {
    match value {
        toml::Value::String(value) => value,
        other => other.to_string(),
    }
}

/// Build the command line for `mise exec -- ...` without spawning it.
pub fn mise_exec_command(argv: &[impl AsRef<str>]) -> Vec<String> {
    ["mise", "exec", "--"]
        .into_iter()
        .map(str::to_string)
        .chain(argv.iter().map(|arg| arg.as_ref().to_string()))
        .collect()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawMiseConfig {
    tools: BTreeMap<String, toml::Value>,
    tasks: BTreeMap<String, RawTask>,
    env: BTreeMap<String, toml::Value>,
    metadata: Option<RawMetadata>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTask {
    description: Option<String>,
    run: Option<toml::Value>,
}

#[derive(Debug, Deserialize)]
struct RawMetadata {
    cockpit: Option<CockpitMetadata>,
}

/// Per-project state persisted outside the project directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectCache {
    pub open_files: Vec<PathBuf>,
    pub active_file: Option<PathBuf>,
    pub left_width: Option<u16>,
    pub right_width: Option<u16>,
    pub recent_files: Vec<PathBuf>,
    pub recent_commands: Vec<String>,
    pub zellij_session_name: Option<String>,
    pub last_selected_mise_task: Option<String>,
    pub terminal_profile: Option<String>,
    pub workspace_layout: Option<String>,
}

impl ProjectCache {
    /// Load a cache file, returning an empty cache if it does not exist.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        match fs::read_to_string(path) {
            Ok(input) => toml::from_str(&input).map_err(ProjectError::Parse),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(ProjectError::Read(err)),
        }
    }

    /// Store a cache file, creating parent directories as needed.
    pub fn store(&self, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(ProjectError::Write)?;
        }
        let output = toml::to_string_pretty(self).map_err(ProjectError::Serialize)?;
        fs::write(path, output).map_err(ProjectError::Write)
    }
}

/// Resolve the default cache path for a project root.
pub fn project_cache_path(root_path: impl AsRef<Path>) -> Result<PathBuf, ProjectError> {
    let dirs = ProjectDirs::from("dev", "CodingCockpit", "cockpit")
        .ok_or(ProjectError::NoCacheDirectory)?;
    Ok(dirs
        .cache_dir()
        .join("projects")
        .join(project_cache_key(root_path.as_ref()))
        .join("state.toml"))
}

fn project_cache_key(root_path: &Path) -> String {
    root_path
        .to_string_lossy()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Default file browser ignores from the Rust implementation plan.
pub const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "__pycache__",
];

/// A lazy project file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTree {
    root_path: PathBuf,
    ignores: Vec<String>,
    root: FileNode,
}

impl FileTree {
    /// Create a tree for `root_path` and load only the root's immediate
    /// children.
    pub fn load(root_path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        Self::load_with_ignores(root_path, DEFAULT_IGNORES)
    }

    /// Create a tree using an explicit ignore list.
    pub fn load_with_ignores(
        root_path: impl AsRef<Path>,
        ignores: &[&str],
    ) -> Result<Self, ProjectError> {
        let root_path = root_path.as_ref().to_path_buf();
        let ignores: Vec<String> = ignores.iter().map(|ignore| ignore.to_string()).collect();
        let children = read_children(&root_path, Path::new(""), &ignores)?;
        let root = FileNode {
            name: root_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("")
                .to_string(),
            path: PathBuf::new(),
            kind: FileNodeKind::Directory,
            children: Some(children),
            expanded: true,
        };

        Ok(Self {
            root_path,
            ignores,
            root,
        })
    }

    /// The root node. Its children are the project root's immediate entries.
    pub fn root(&self) -> &FileNode {
        &self.root
    }

    /// Absolute path of the project root this tree was loaded from.
    pub fn root_path(&self) -> &Path {
        &self.root_path
    }

    /// Find a node by project-relative path.
    pub fn node(&self, relative_path: impl AsRef<Path>) -> Option<&FileNode> {
        let relative_path = normalize_relative(relative_path.as_ref());
        find_node(&self.root, &relative_path)
    }

    /// Expand a directory, loading its children on first expansion.
    pub fn expand(&mut self, relative_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        let root_path = self.root_path.clone();
        let ignores = self.ignores.clone();
        let Some(node) = find_node_mut(&mut self.root, &relative_path) else {
            return Err(ProjectError::NotFound(relative_path));
        };
        if node.kind != FileNodeKind::Directory {
            return Err(ProjectError::NotDirectory(relative_path));
        }
        if node.children.is_none() {
            node.children = Some(read_children(&root_path, &node.path, &ignores)?);
        }
        node.expanded = true;
        Ok(())
    }

    /// Collapse a directory without discarding already-loaded children.
    pub fn collapse(&mut self, relative_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        let Some(node) = find_node_mut(&mut self.root, &relative_path) else {
            return Err(ProjectError::NotFound(relative_path));
        };
        if node.kind != FileNodeKind::Directory {
            return Err(ProjectError::NotDirectory(relative_path));
        }
        node.expanded = false;
        Ok(())
    }

    /// Create a file and refresh its loaded parent.
    pub fn create_file(&mut self, relative_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        let absolute = self.root_path.join(&relative_path);
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent).map_err(ProjectError::Write)?;
        }
        fs::File::create(&absolute).map_err(ProjectError::Write)?;
        self.refresh_parent(&relative_path)
    }

    /// Create a directory and refresh its loaded parent.
    pub fn create_dir(&mut self, relative_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        fs::create_dir_all(self.root_path.join(&relative_path)).map_err(ProjectError::Write)?;
        self.refresh_parent(&relative_path)
    }

    /// Rename a path within the same parent directory and refresh that parent.
    pub fn rename(
        &mut self,
        relative_path: impl AsRef<Path>,
        new_name: &str,
    ) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        let new_relative_path = parent.join(new_name);
        fs::rename(
            self.root_path.join(&relative_path),
            self.root_path.join(&new_relative_path),
        )
        .map_err(ProjectError::Write)?;
        self.refresh(parent).map(|_| ())
    }

    /// Delete a file or directory and refresh its loaded parent.
    pub fn delete(&mut self, relative_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let relative_path = normalize_relative(relative_path.as_ref());
        let absolute = self.root_path.join(&relative_path);
        let metadata = fs::metadata(&absolute).map_err(ProjectError::Read)?;
        if metadata.is_dir() {
            fs::remove_dir_all(&absolute).map_err(ProjectError::Write)?;
        } else {
            fs::remove_file(&absolute).map_err(ProjectError::Write)?;
        }
        self.refresh_parent(&relative_path)
    }

    fn refresh_parent(&mut self, relative_path: &Path) -> Result<(), ProjectError> {
        let mut parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
        loop {
            if self.refresh(parent)? {
                return Ok(());
            }
            let Some(next_parent) = parent.parent() else {
                return Ok(());
            };
            parent = next_parent;
        }
    }

    fn refresh(&mut self, relative_path: &Path) -> Result<bool, ProjectError> {
        let root_path = self.root_path.clone();
        let ignores = self.ignores.clone();
        let Some(node) = find_node_mut(&mut self.root, relative_path) else {
            return Ok(false);
        };
        if node.children.is_some() {
            node.children = Some(read_children(&root_path, &node.path, &ignores)?);
        }
        Ok(true)
    }
}

/// One file tree node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileNode {
    pub name: String,
    pub path: PathBuf,
    pub kind: FileNodeKind,
    children: Option<Vec<FileNode>>,
    pub expanded: bool,
}

impl FileNode {
    /// Loaded children, if this directory has been loaded.
    pub fn children(&self) -> Option<&[FileNode]> {
        self.children.as_deref()
    }

    /// True when this directory's children have been loaded.
    pub fn children_loaded(&self) -> bool {
        self.children.is_some()
    }
}

/// File tree node kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileNodeKind {
    File,
    Directory,
}

fn read_children(
    root_path: &Path,
    relative_path: &Path,
    ignores: &[String],
) -> Result<Vec<FileNode>, ProjectError> {
    let mut children = Vec::new();
    for entry in fs::read_dir(root_path.join(relative_path)).map_err(ProjectError::Read)? {
        let entry = entry.map_err(ProjectError::Read)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if ignores.iter().any(|ignore| ignore == &name) {
            continue;
        }
        let file_type = entry.file_type().map_err(ProjectError::Read)?;
        let kind = if file_type.is_dir() {
            FileNodeKind::Directory
        } else {
            FileNodeKind::File
        };
        children.push(FileNode {
            path: relative_path.join(&name),
            name,
            kind,
            children: None,
            expanded: false,
        });
    }

    children.sort_by(|a, b| {
        b.kind
            .cmp(&a.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(children)
}

/// Recursively collect every file under `root`, as project-relative paths,
/// skipping [`DEFAULT_IGNORES`] directories. The result is sorted, so it is a
/// stable index for fuzzy file open (spec §23 v0.2). Symlinks are not followed.
pub fn walk_project_files(root: impl AsRef<Path>) -> Result<Vec<PathBuf>, ProjectError> {
    let root = root.as_ref();
    let mut files = Vec::new();
    walk_files_into(root, Path::new(""), &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_files_into(
    root: &Path,
    relative: &Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), ProjectError> {
    for entry in fs::read_dir(root.join(relative)).map_err(ProjectError::Read)? {
        let entry = entry.map_err(ProjectError::Read)?;
        let name = entry.file_name().to_string_lossy().to_string();
        if DEFAULT_IGNORES.contains(&name.as_str()) {
            continue;
        }
        let child = relative.join(&name);
        // `file_type` does not follow symlinks, so symlink loops cannot occur.
        let file_type = entry.file_type().map_err(ProjectError::Read)?;
        if file_type.is_dir() {
            walk_files_into(root, &child, out)?;
        } else if file_type.is_file() {
            out.push(child);
        }
    }
    Ok(())
}

fn normalize_relative(path: &Path) -> PathBuf {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(PathBuf::from(part)),
            _ => None,
        })
        .collect()
}

fn find_node<'a>(node: &'a FileNode, relative_path: &Path) -> Option<&'a FileNode> {
    if node.path == relative_path {
        return Some(node);
    }
    let children = node.children.as_ref()?;
    children
        .iter()
        .find_map(|child| find_node(child, relative_path))
}

fn find_node_mut<'a>(node: &'a mut FileNode, relative_path: &Path) -> Option<&'a mut FileNode> {
    if node.path == relative_path {
        return Some(node);
    }
    let children = node.children.as_mut()?;
    children
        .iter_mut()
        .find_map(|child| find_node_mut(child, relative_path))
}

/// Project crate error.
#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("failed to read project data: {0}")]
    Read(#[source] io::Error),
    #[error("failed to write project data: {0}")]
    Write(#[source] io::Error),
    #[error("failed to parse project data: {0}")]
    Parse(#[source] toml::de::Error),
    #[error("failed to serialize project data: {0}")]
    Serialize(#[source] toml::ser::Error),
    #[error("could not resolve an OS cache directory")]
    NoCacheDirectory,
    #[error("project path not found: {0}")]
    NotFound(PathBuf),
    #[error("project path is not a directory: {0}")]
    NotDirectory(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_project_files_collects_sorted_relative_paths_skipping_ignores() {
        let files = walk_project_files(cockpit_testkit::fixture_path("file-tree")).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(
            names,
            [
                "README.md",
                "src/lib.rs",
                "src/nested/mod.rs",
                "tests/basic.rs",
            ]
        );
    }

    const MISE_SAMPLE: &str = r#"
[tools]
rust = "1.88"
python = "3.13"
node = "24"

[env]
APP_ENV = "development"

[tasks.dev]
description = "Run development server"
run = "uv run fastapi dev"

[tasks.test]
description = "Run tests"
run = "cargo nextest run"

[metadata.cockpit]
name = "Geotech Platform"
default_task = "dev"
terminal_workspace = "zellij"
zellij_layout = ".config/zellij/dev.kdl"
"#;

    #[test]
    fn parses_mise_tools_tasks_env_and_metadata() {
        let project = parse_mise_toml(MISE_SAMPLE).unwrap();

        assert!(project.detected);
        assert_eq!(
            project.tools,
            vec![
                Tool {
                    name: "node".to_string(),
                    version: "24".to_string()
                },
                Tool {
                    name: "python".to_string(),
                    version: "3.13".to_string()
                },
                Tool {
                    name: "rust".to_string(),
                    version: "1.88".to_string()
                }
            ]
        );
        assert_eq!(
            project.tasks,
            vec![
                Task {
                    name: "dev".to_string(),
                    description: Some("Run development server".to_string()),
                    run: Some("uv run fastapi dev".to_string())
                },
                Task {
                    name: "test".to_string(),
                    description: Some("Run tests".to_string()),
                    run: Some("cargo nextest run".to_string())
                }
            ]
        );
        assert_eq!(
            project.env,
            vec![EnvVar {
                name: "APP_ENV".to_string(),
                value: "development".to_string()
            }]
        );
        assert_eq!(
            project.metadata.unwrap().zellij_layout,
            Some(PathBuf::from(".config/zellij/dev.kdl"))
        );
    }

    #[test]
    fn detects_mise_fixture_as_strongest_signal() {
        let root = workspace_root().join("tests/fixtures/mise-basic");
        let project = detect_project(root).unwrap();

        assert!(project.detected());
        assert_eq!(project.strongest_signal, Some(ProjectSignalKind::Mise));
        assert!(project.mise.detected);
        assert_eq!(
            project
                .mise
                .tasks
                .iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            vec!["lint", "test"]
        );
        assert_eq!(
            project
                .mise
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["rust"]
        );
    }

    #[test]
    fn detects_non_mise_project_signal() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::write(tempdir.path().join("Cargo.toml"), "[package]\nname = \"x\"").unwrap();

        let project = detect_project(tempdir.path()).unwrap();
        assert_eq!(project.strongest_signal, Some(ProjectSignalKind::Rust));
        assert!(!project.mise.detected);
    }

    #[test]
    fn missing_mise_degrades_without_error() {
        let tempdir = tempfile::tempdir().unwrap();
        let project = detect_mise_project(tempdir.path()).unwrap();
        assert_eq!(project, MiseProject::default());
    }

    #[test]
    fn builds_mise_exec_command() {
        assert_eq!(
            mise_exec_command(&["cargo", "test"]),
            ["mise", "exec", "--", "cargo", "test"]
        );
    }

    #[test]
    fn cache_round_trips() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("state.toml");
        let cache = ProjectCache {
            active_file: Some(PathBuf::from("src/main.rs")),
            recent_commands: vec!["Editor: Save".to_string()],
            zellij_session_name: Some("my-project".to_string()),
            terminal_profile: Some("project-zellij".to_string()),
            ..ProjectCache::default()
        };

        cache.store(&path).unwrap();
        assert_eq!(ProjectCache::load(&path).unwrap(), cache);
    }

    #[test]
    fn cache_path_uses_stable_project_key() {
        let path = project_cache_path("/tmp/My Project").unwrap();
        assert!(path.ends_with("projects/_tmp_my_project/state.toml"));
    }

    #[test]
    fn file_tree_loads_root_only_and_filters_ignores() {
        let root = workspace_root().join("tests/fixtures/file-tree");
        let tree = FileTree::load(root).unwrap();
        let names = tree
            .root()
            .children()
            .unwrap()
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["src", "tests", "README.md"]);
        assert!(tree.node("src").unwrap().children().is_none());
        assert!(tree.node("target").is_none());
        assert!(tree.node("node_modules").is_none());
    }

    #[test]
    fn file_tree_expands_directories_lazily() {
        let root = workspace_root().join("tests/fixtures/file-tree");
        let mut tree = FileTree::load(root).unwrap();

        assert!(!tree.node("src").unwrap().children_loaded());
        tree.expand("src").unwrap();

        let names = tree
            .node("src")
            .unwrap()
            .children()
            .unwrap()
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["nested", "lib.rs"]);
        assert!(!tree.node("src/nested").unwrap().children_loaded());
    }

    #[test]
    fn file_tree_create_rename_and_delete_file() {
        let tempdir = tempfile::tempdir().unwrap();
        fs::create_dir(tempdir.path().join("src")).unwrap();
        let mut tree = FileTree::load(tempdir.path()).unwrap();
        tree.expand("src").unwrap();

        tree.create_file("src/new.rs").unwrap();
        assert!(tempdir.path().join("src/new.rs").exists());
        assert!(tree.node("src/new.rs").is_some());

        tree.rename("src/new.rs", "renamed.rs").unwrap();
        assert!(!tempdir.path().join("src/new.rs").exists());
        assert!(tempdir.path().join("src/renamed.rs").exists());
        assert!(tree.node("src/renamed.rs").is_some());

        tree.delete("src/renamed.rs").unwrap();
        assert!(!tempdir.path().join("src/renamed.rs").exists());
        assert!(tree.node("src/renamed.rs").is_none());
    }

    #[test]
    fn file_tree_create_and_delete_directory() {
        let tempdir = tempfile::tempdir().unwrap();
        let mut tree = FileTree::load(tempdir.path()).unwrap();

        tree.create_dir("src/nested").unwrap();
        assert!(tempdir.path().join("src/nested").is_dir());
        assert!(tree.node("src").is_some());

        tree.expand("src").unwrap();
        assert!(tree.node("src/nested").is_some());

        tree.delete("src").unwrap();
        assert!(!tempdir.path().join("src").exists());
        assert!(tree.node("src").is_none());
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf()
    }
}
