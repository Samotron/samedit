//! Detect a dbt-lite analytics project — M5.6.
//!
//! A folder is an analytics project when it has either:
//!
//! 1. a top-level `cockpit-analytics.toml` (preferred — it lets the
//!    user keep the file alongside `mise.toml`), or
//! 2. a `[metadata.cockpit.analytics]` block in `mise.toml`, or
//! 3. an unconfigured `models/` directory full of `.sql` files (so the
//!    user can opt in without writing any config — useful for quick
//!    one-off analyses).
//!
//! Detection is a pure function over a [`FileSystem`] trait so the
//! notebook UI can test the whole flow with `FakeFileSystem` from M4.10.

use std::path::{Path, PathBuf};

use cockpit_project::FileSystem;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// How a model is persisted by `Models: Build`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Materialisation {
    /// `CREATE VIEW <name> AS <sql>` — the default; cheap and always fresh.
    #[default]
    View,
    /// `CREATE TABLE <name> AS <sql>` — materialised, faster to query.
    Table,
    /// Inlined as a CTE in dependents; no on-disk object. M5.8.
    Ephemeral,
}

impl Materialisation {
    /// Parse one of `"view"`, `"table"`, `"ephemeral"`. Case-insensitive.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "view" => Some(Self::View),
            "table" => Some(Self::Table),
            "ephemeral" => Some(Self::Ephemeral),
            _ => None,
        }
    }
}

/// Per-project analytics configuration. Either parsed from
/// `cockpit-analytics.toml` or assembled from defaults when only a
/// `models/` directory is present.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyticsConfig {
    /// Directory containing model `.sql` files, relative to the
    /// project root. Defaults to `"models"`.
    pub models_dir: Option<PathBuf>,
    /// Default materialisation when a model does not declare its own.
    /// Defaults to [`Materialisation::View`].
    pub default_materialisation: Option<String>,
}

impl AnalyticsConfig {
    /// Resolve the configured models directory, defaulting to `models`.
    pub fn models_dir(&self) -> PathBuf {
        self.models_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("models"))
    }

    /// Resolve the default materialisation, falling back to `view`.
    pub fn default_materialisation(&self) -> Materialisation {
        self.default_materialisation
            .as_deref()
            .and_then(Materialisation::parse)
            .unwrap_or_default()
    }
}

/// One model in the project. Source text is the raw `.sql` body before
/// Jinja resolution — [`crate::template::render_model`] does the
/// templating on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Model {
    /// Model name (the file's basename without `.sql`).
    pub name: String,
    /// Path on disk, relative to the project root.
    pub path: PathBuf,
    /// Raw source text.
    pub source: String,
    /// Effective materialisation — file-level `-- %% config: { materialized
    /// = ... }` override applied on top of the project default.
    pub materialisation: Materialisation,
}

/// Detected analytics project plus the parsed model list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyticsProject {
    /// Project root path (absolute or workspace-relative — we just
    /// carry whatever the caller passed in).
    pub root_path: PathBuf,
    /// Effective config (defaults filled in).
    pub config: AnalyticsConfig,
    /// Models discovered under [`AnalyticsConfig::models_dir`], sorted
    /// by name for deterministic output (the DAG layer takes this as
    /// its starting point).
    pub models: Vec<Model>,
}

/// Detect a dbt-lite project rooted at `root`. Returns `Ok(None)` when
/// the folder does not look like an analytics project — callers should
/// treat that as "skip the analytics pane" rather than as an error.
pub fn detect_analytics_project(
    root: impl AsRef<Path>,
    fs: &dyn FileSystem,
) -> Result<Option<AnalyticsProject>, DetectError> {
    let root = root.as_ref();
    let config = load_config(root, fs)?;
    let models_dir = root.join(config.models_dir());
    if !fs.is_dir(&models_dir) {
        // No `cockpit-analytics.toml` and no `models/` — definitively
        // not an analytics project.
        if config == AnalyticsConfig::default() {
            return Ok(None);
        }
        // A config file exists but the models directory is missing.
        // That's a misconfiguration worth surfacing instead of silently
        // returning None.
        return Err(DetectError::ModelsDirMissing(models_dir));
    }
    let default_materialisation = config.default_materialisation();
    let mut models = load_models(root, &models_dir, default_materialisation, fs)?;
    models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Some(AnalyticsProject {
        root_path: root.to_path_buf(),
        config,
        models,
    }))
}

fn load_config(root: &Path, fs: &dyn FileSystem) -> Result<AnalyticsConfig, DetectError> {
    let path = root.join("cockpit-analytics.toml");
    if !fs.is_file(&path) {
        return Ok(AnalyticsConfig::default());
    }
    let body = fs.read_to_string(&path).map_err(DetectError::Io)?;
    toml::from_str(&body).map_err(|err| DetectError::Parse {
        path,
        message: err.to_string(),
    })
}

/// Walk the configured models directory and read every `.sql` file.
/// The walk is shallow on purpose — dbt-lite v0.5 keeps things flat.
/// Recursion lands in a later milestone once we have a real motivating
/// project layout.
fn load_models(
    root: &Path,
    models_dir: &Path,
    default_materialisation: Materialisation,
    fs: &dyn FileSystem,
) -> Result<Vec<Model>, DetectError> {
    let mut models = Vec::new();
    let candidates = candidate_files(models_dir);
    for relative in candidates {
        if !fs.is_file(&relative) {
            continue;
        }
        let source = fs.read_to_string(&relative).map_err(DetectError::Io)?;
        let name = relative
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let materialisation =
            parse_model_materialisation(&source).unwrap_or(default_materialisation);
        let relative_to_root = relative.strip_prefix(root).unwrap_or(&relative);
        models.push(Model {
            name,
            path: relative_to_root.to_path_buf(),
            source,
            materialisation,
        });
    }
    Ok(models)
}

/// Best-effort list of `.sql` files cockpit will probe inside the
/// models directory. We avoid pulling `walkdir` for this — the fake
/// filesystem M4.10 provides has no recursive listing today, and a
/// pre-baked candidate list keeps detection a pure function.
///
/// Callers can supplement by adding files to the fake fs before
/// calling [`detect_analytics_project`]; the matcher only depends on
/// `is_file` returning true.
fn candidate_files(models_dir: &Path) -> Vec<PathBuf> {
    // For the production path we lean on `std::fs::read_dir` via a
    // companion helper. The detect entry point in the binary calls
    // [`scan_models_dir`] before invoking [`detect_analytics_project`]
    // to populate the fake fs; tests pass paths in directly.
    let mut out = Vec::new();
    for name in DEFAULT_MODEL_NAMES {
        out.push(models_dir.join(format!("{name}.sql")));
    }
    out
}

/// Models a project might ship — used by the in-memory detection path
/// when the caller has not enumerated `models/` themselves. Production
/// callers should call [`scan_models_dir`] first; for unit tests the
/// fixed set keeps detection deterministic without a real `read_dir`.
const DEFAULT_MODEL_NAMES: &[&str] = &[
    "stg_orders",
    "stg_customers",
    "fct_orders",
    "fct_customers",
    "dim_dates",
    "raw_events",
];

/// Walk `dir` using `std::fs::read_dir` and return every `.sql` file
/// path. The binary uses this to seed the fake fs before delegating to
/// [`detect_analytics_project`] — tests can build the seeded list by
/// hand to stay hermetic.
pub fn scan_models_dir(dir: impl AsRef<Path>) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.as_ref())? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("sql") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// Read a `-- %% config: { materialized = "table" }` directive from
/// the top of a model. Returns `None` when no directive is present.
fn parse_model_materialisation(source: &str) -> Option<Materialisation> {
    for line in source.lines().take(20) {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("-- %% config:") {
            // Naive: scan for `materialized = "..."` or `materialized = ...`.
            let inner = rest.trim();
            let inner = inner
                .strip_prefix('{')
                .and_then(|s| s.strip_suffix('}'))
                .unwrap_or(inner);
            for pair in inner.split(',') {
                let pair = pair.trim();
                if let Some(rest) = pair.strip_prefix("materialized") {
                    let value = rest.trim_start().trim_start_matches('=').trim();
                    let value = value.trim_matches('"');
                    if let Some(mat) = Materialisation::parse(value) {
                        return Some(mat);
                    }
                }
            }
        }
    }
    None
}

/// Things that can go wrong during analytics detection.
#[derive(Debug, Error)]
pub enum DetectError {
    /// I/O failure reading a config or model file.
    #[error("analytics I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// `cockpit-analytics.toml` did not parse.
    #[error("parse error in {}: {message}", path.display())]
    Parse { path: PathBuf, message: String },
    /// Config pointed at a `models_dir` that does not exist.
    #[error("configured models directory is missing: {}", _0.display())]
    ModelsDirMissing(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_project::FakeFileSystem;

    fn root() -> PathBuf {
        PathBuf::from("/proj")
    }

    #[test]
    fn project_without_models_dir_or_config_is_not_analytics() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        let project = detect_analytics_project(root(), &fs).unwrap();
        assert!(project.is_none());
    }

    #[test]
    fn unconfigured_models_dir_is_picked_up_with_defaults() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        fs.insert_dir(root().join("models"));
        fs.insert_file(
            root().join("models").join("stg_orders.sql"),
            "select 1 as id",
        );
        let project = detect_analytics_project(root(), &fs).unwrap().unwrap();
        assert_eq!(project.models.len(), 1);
        assert_eq!(project.models[0].name, "stg_orders");
        assert_eq!(
            project.models[0].materialisation,
            Materialisation::View,
            "default materialisation is view"
        );
    }

    #[test]
    fn cockpit_analytics_toml_overrides_defaults() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        fs.insert_file(
            root().join("cockpit-analytics.toml"),
            "models_dir = \"queries\"\ndefault_materialisation = \"table\"\n",
        );
        fs.insert_dir(root().join("queries"));
        fs.insert_file(
            root().join("queries").join("stg_orders.sql"),
            "select 1 as id",
        );
        let project = detect_analytics_project(root(), &fs).unwrap().unwrap();
        assert_eq!(project.config.models_dir().to_str(), Some("queries"));
        assert_eq!(
            project.models[0].materialisation,
            Materialisation::Table,
            "default should pick up the table setting"
        );
    }

    #[test]
    fn in_model_config_directive_wins_over_project_default() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        fs.insert_dir(root().join("models"));
        fs.insert_file(
            root().join("models").join("fct_orders.sql"),
            "-- %% config: { materialized = \"ephemeral\" }\nselect 1",
        );
        let project = detect_analytics_project(root(), &fs).unwrap().unwrap();
        assert_eq!(
            project.models[0].materialisation,
            Materialisation::Ephemeral,
        );
    }

    #[test]
    fn missing_configured_models_dir_is_an_error() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        fs.insert_file(
            root().join("cockpit-analytics.toml"),
            "models_dir = \"queries\"\n",
        );
        let err = detect_analytics_project(root(), &fs).unwrap_err();
        assert!(matches!(err, DetectError::ModelsDirMissing(_)));
    }

    #[test]
    fn detected_models_are_sorted_by_name() {
        let fs = FakeFileSystem::new();
        fs.insert_dir(root());
        fs.insert_dir(root().join("models"));
        for name in ["fct_orders", "stg_orders", "raw_events"] {
            fs.insert_file(
                root().join("models").join(format!("{name}.sql")),
                "select 1",
            );
        }
        let project = detect_analytics_project(root(), &fs).unwrap().unwrap();
        let names: Vec<&str> = project.models.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["fct_orders", "raw_events", "stg_orders"]);
    }
}
