//! Loading an [`OrgRoot`] from disk, and deriving a [`NowStamp`] from the
//! system clock.
//!
//! This is binary glue (the headless [`crate::app`] never touches the
//! filesystem or the clock). `load_root` is still unit-tested against a
//! tempdir; `now_stamp` is clock-bound and exercised only at runtime.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_org::date::{civil_from_days, weekday_abbr};
use cockpit_org::{NowStamp, OrgConfig, OrgRoot, OrgTime};

/// Read and parse `org.toml` at `path`. A missing file is not an error — the
/// jot app runs on defaults (no templates, `TODO`/`DONE` workflow) — but a
/// malformed file is, so a typo surfaces loudly rather than silently dropping
/// the user's capture templates.
pub fn load_config(path: impl AsRef<Path>) -> io::Result<OrgConfig> {
    match fs::read_to_string(path.as_ref()) {
        Ok(source) => OrgConfig::from_toml_str(&source)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(OrgConfig::default()),
        Err(e) => Err(e),
    }
}

/// The default config path, `$HOME/.config/cockpit/org.toml`. `None` when
/// `HOME` is unset.
pub fn default_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config/cockpit/org.toml"))
}

/// Resolve the org root directory. An explicit `cli` override wins; otherwise
/// the config's `root` (with a leading `~` expanded against `$HOME`); otherwise
/// the plan default of `~/org`.
pub fn resolve_org_root(cli: Option<PathBuf>, config: &OrgConfig) -> PathBuf {
    if let Some(dir) = cli {
        return dir;
    }
    if let Some(root) = &config.root {
        return expand_tilde(root);
    }
    default_org_root()
}

/// Expand a leading `~` / `~/` against `$HOME`. Anything else (absolute or
/// relative paths, `$VAR`s we don't handle) passes through verbatim.
fn expand_tilde(path: &str) -> PathBuf {
    let home = std::env::var_os("HOME");
    match (path, path.strip_prefix("~/"), home) {
        ("~", _, Some(home)) => PathBuf::from(home),
        (_, Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// The plan default org root, `~/org` (or bare `org` when `HOME` is unset).
pub fn default_org_root() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("org"),
        None => PathBuf::from("org"),
    }
}

/// Walk `dir` (non-recursively) and parse every `*.org` file into an
/// [`OrgRoot`], using the workflow from `config`.
pub fn load_root(dir: impl AsRef<Path>, config: &OrgConfig) -> io::Result<OrgRoot> {
    let dir = dir.as_ref();
    let mut root = OrgRoot::with_keywords(dir, config.keywords());
    if !dir.exists() {
        return Ok(root);
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("org") {
            let source = fs::read_to_string(&path)?;
            root.insert(path, source);
        }
    }
    Ok(root)
}

/// The current moment as a [`NowStamp`], in UTC. (Local-timezone handling is a
/// follow-up; capture/agenda only need a consistent calendar reference.)
pub fn now_stamp() -> NowStamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let date = civil_from_days(days);
    let sod = secs % 86_400;
    let time = OrgTime::new((sod / 3600) as u8, ((sod % 3600) / 60) as u8);
    NowStamp::new(date, time, weekday_abbr(date))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_only_org_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("inbox.org"), "* TODO a\n").unwrap();
        fs::write(dir.path().join("notes.org"), "* b\n").unwrap();
        fs::write(dir.path().join("README.md"), "not org\n").unwrap();

        let root = load_root(dir.path(), &cfg()).unwrap();
        assert_eq!(root.files.len(), 2);
        assert!(root.file(dir.path().join("inbox.org")).is_some());
    }

    #[test]
    fn missing_dir_is_empty_root() {
        let root = load_root("/nonexistent/org/root", &cfg()).unwrap();
        assert!(root.files.is_empty());
    }

    fn cfg() -> OrgConfig {
        OrgConfig {
            root: None,
            default_todo_keywords: vec!["TODO".into(), "DONE".into()],
            capture: Vec::new(),
        }
    }

    #[test]
    fn load_config_reads_templates_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org.toml");
        fs::write(
            &path,
            "[org]\nroot = \"~/notes\"\n\n[[org.capture]]\n\
             key = \"t\"\nname = \"Todo\"\n\
             target = { file = \"inbox.org\", under = \"Tasks\" }\n\
             template = \"* TODO %?\"\n",
        )
        .unwrap();

        let config = load_config(&path).unwrap();
        assert_eq!(config.root.as_deref(), Some("~/notes"));
        assert_eq!(config.template("t").unwrap().name, "Todo");
    }

    #[test]
    fn load_config_missing_file_is_default() {
        let config = load_config("/nonexistent/org.toml").unwrap();
        assert_eq!(config, OrgConfig::default());
    }

    #[test]
    fn load_config_malformed_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org.toml");
        fs::write(&path, "[org\nbroken").unwrap();
        assert!(load_config(&path).is_err());
    }

    #[test]
    fn resolve_root_prefers_cli_then_config() {
        // Use an absolute config root so the result is independent of `$HOME`
        // (and so this test never mutates the shared environment).
        let with_root = OrgConfig {
            root: Some("/srv/notes".into()),
            ..OrgConfig::default()
        };

        // CLI override wins outright.
        assert_eq!(
            resolve_org_root(Some(PathBuf::from("/explicit")), &with_root),
            PathBuf::from("/explicit"),
        );

        // Else the config's `root`.
        assert_eq!(
            resolve_org_root(None, &with_root),
            PathBuf::from("/srv/notes"),
        );
    }

    #[test]
    fn expand_tilde_passes_through_non_tilde_paths() {
        // Absolute and relative paths are never touched, regardless of `$HOME`.
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/path"), PathBuf::from("rel/path"));
    }

    #[test]
    fn expand_tilde_uses_home_when_set() {
        // Read the live `$HOME` rather than mutating it: when present, a leading
        // `~/` resolves under it; the no-HOME branch is covered by the
        // pass-through test above on machines without one.
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                expand_tilde("~/org"),
                PathBuf::from(home).join("org"),
            );
        }
    }
}
