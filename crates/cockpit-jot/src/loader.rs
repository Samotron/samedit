//! Loading an [`OrgRoot`] from disk, and deriving a [`NowStamp`] from the
//! system clock.
//!
//! This is binary glue (the headless [`crate::app`] never touches the
//! filesystem or the clock). `load_root` is still unit-tested against a
//! tempdir; `now_stamp` is clock-bound and exercised only at runtime.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_org::date::{civil_from_days, weekday_abbr};
use cockpit_org::{NowStamp, OrgConfig, OrgRoot, OrgTime};

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
}
