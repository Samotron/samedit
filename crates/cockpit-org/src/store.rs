//! In-memory store of every `.org` file under a root folder.
//!
//! `OrgRoot` is pure data: it is handed `(path, source)` pairs and parses them.
//! The actual directory walk, the `notify` watcher, and writing files back to
//! disk live in the jot binary (M12.6) and the cockpit integration (M12.7),
//! behind the `cockpit-project::env` filesystem seam — keeping this crate
//! hermetic and free of `std::fs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::keywords::Keywords;
use crate::model::{Heading, OrgFile};
use crate::parse::parse_file_with;

/// The default folder layout written on first launch when the root is empty.
///
/// `(relative filename, seed content)`. The binary writes these only if the
/// root directory contains no `.org` files — it never overwrites.
pub const DEFAULT_LAYOUT: &[(&str, &str)] = &[
    ("inbox.org", "#+TITLE: Inbox\n"),
    ("tasks.org", "#+TITLE: Tasks\n"),
    ("notes.org", "#+TITLE: Notes\n"),
    ("journal.org", "#+TITLE: Journal\n"),
];

/// Every `.org` file under a configured root, parsed and indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgRoot {
    /// Root directory the files were loaded from.
    pub root_dir: PathBuf,
    /// Parsed files, keyed by path, ordered for deterministic iteration.
    pub files: BTreeMap<PathBuf, OrgFile>,
    /// TODO-keyword workflow applied when (re)parsing files.
    keywords: Keywords,
}

impl OrgRoot {
    /// An empty root with the default `TODO | DONE` workflow.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        OrgRoot {
            root_dir: root_dir.into(),
            files: BTreeMap::new(),
            keywords: Keywords::default(),
        }
    }

    /// An empty root with a custom workflow.
    pub fn with_keywords(root_dir: impl Into<PathBuf>, keywords: Keywords) -> Self {
        OrgRoot {
            root_dir: root_dir.into(),
            files: BTreeMap::new(),
            keywords,
        }
    }

    /// Build a root from an iterator of `(path, source)` pairs.
    pub fn from_files<I, P, S>(root_dir: impl Into<PathBuf>, files: I) -> Self
    where
        I: IntoIterator<Item = (P, S)>,
        P: Into<PathBuf>,
        S: Into<String>,
    {
        let mut root = Self::new(root_dir);
        for (path, source) in files {
            root.insert(path, source);
        }
        root
    }

    /// The workflow used when parsing files in this root.
    pub fn keywords(&self) -> &Keywords {
        &self.keywords
    }

    /// Parse `source` and insert (or replace) the file at `path`.
    pub fn insert(&mut self, path: impl Into<PathBuf>, source: impl Into<String>) {
        let path = path.into();
        let file = parse_file_with(&path, source, &self.keywords);
        self.files.insert(path, file);
    }

    /// Drop the file at `path`, if present.
    pub fn remove(&mut self, path: impl AsRef<Path>) -> Option<OrgFile> {
        self.files.remove(path.as_ref())
    }

    /// Look up a parsed file by path.
    pub fn file(&self, path: impl AsRef<Path>) -> Option<&OrgFile> {
        self.files.get(path.as_ref())
    }

    /// Pre-order iterator over `(file, heading)` for every heading in the root,
    /// in path order then document order.
    pub fn iter_headings(&self) -> impl Iterator<Item = (&OrgFile, &Heading)> {
        self.files
            .values()
            .flat_map(|f| f.iter_headings().map(move |h| (f, h)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_iterate() {
        let mut root = OrgRoot::new("/org");
        root.insert("/org/a.org", "* TODO one\n* two\n");
        root.insert("/org/b.org", "* three\n");

        let titles: Vec<_> = root
            .iter_headings()
            .map(|(_, h)| h.title.as_str())
            .collect();
        // BTreeMap orders a.org before b.org.
        assert_eq!(titles, ["one", "two", "three"]);
        assert_eq!(root.files.len(), 2);
    }

    #[test]
    fn insert_replaces_existing() {
        let mut root = OrgRoot::new("/org");
        root.insert("/org/a.org", "* one\n");
        root.insert("/org/a.org", "* one\n* two\n");
        assert_eq!(root.file("/org/a.org").unwrap().headings.len(), 2);
        assert_eq!(root.files.len(), 1);
    }

    #[test]
    fn custom_keywords_flow_through() {
        let kw = Keywords::from_sequence(["TODO", "NEXT", "DONE"]);
        let mut root = OrgRoot::with_keywords("/org", kw);
        root.insert("/org/a.org", "* NEXT task\n");
        let h = &root.file("/org/a.org").unwrap().headings[0];
        assert_eq!(h.todo_keyword.as_deref(), Some("NEXT"));
        assert_eq!(h.title, "task");
    }
}
