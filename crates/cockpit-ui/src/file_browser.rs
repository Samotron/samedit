//! File-browser view-model (M1.15).
//!
//! A selection + expansion layer over the lazy [`FileTree`] from
//! `cockpit-project`. The browser flattens the visible tree into a list of
//! [`FileRow`]s, tracks a selection cursor, and turns keyboard navigation into
//! either tree mutations (expand/collapse) or an open-file intent. Like the
//! rest of `cockpit-ui` it is plain data and fully testable without a window.

use std::path::{Path, PathBuf};

use cockpit_project::{FileNode, FileNodeKind, FileTree, ProjectError};

/// One visible row in the flattened file-browser list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// Project-relative path of the entry.
    pub path: PathBuf,
    /// Display name (the final path component).
    pub name: String,
    /// Indentation depth; entries directly under the project root are depth 0.
    pub depth: usize,
    /// Entry kind.
    pub kind: FileNodeKind,
    /// For directories: whether the directory is currently expanded.
    pub expanded: bool,
}

impl FileRow {
    /// True when this row is a directory.
    pub fn is_dir(&self) -> bool {
        self.kind == FileNodeKind::Directory
    }
}

/// Outcome of [`FileBrowser::activate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileBrowserAction {
    /// A directory was expanded or collapsed; the row list changed.
    Toggled,
    /// A file was activated; the caller should open this absolute path.
    OpenFile(PathBuf),
    /// There was nothing to activate (the tree is empty).
    Nothing,
}

/// Selection + expansion view-model over a lazy [`FileTree`].
#[derive(Debug)]
pub struct FileBrowser {
    tree: FileTree,
    rows: Vec<FileRow>,
    selected: usize,
}

impl FileBrowser {
    /// Build a browser over an already-loaded file tree.
    pub fn new(tree: FileTree) -> Self {
        let mut browser = Self {
            tree,
            rows: Vec::new(),
            selected: 0,
        };
        browser.rebuild(None);
        browser
    }

    /// Flattened visible rows, in display order.
    pub fn rows(&self) -> &[FileRow] {
        &self.rows
    }

    /// Index of the selected row.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The selected row, if any.
    pub fn selected(&self) -> Option<&FileRow> {
        self.rows.get(self.selected)
    }

    /// True when there are no visible entries.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Borrow the underlying file tree.
    pub fn tree(&self) -> &FileTree {
        &self.tree
    }

    /// Move the selection up one row, saturating at the top.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down one row, saturating at the bottom.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    /// Activate the selected row: expand/collapse a directory, or report the
    /// absolute path of a file to open.
    pub fn activate(&mut self) -> Result<FileBrowserAction, ProjectError> {
        let Some(row) = self.rows.get(self.selected).cloned() else {
            return Ok(FileBrowserAction::Nothing);
        };
        match row.kind {
            FileNodeKind::Directory => {
                if row.expanded {
                    self.tree.collapse(&row.path)?;
                } else {
                    self.tree.expand(&row.path)?;
                }
                self.rebuild(Some(&row.path));
                Ok(FileBrowserAction::Toggled)
            }
            FileNodeKind::File => Ok(FileBrowserAction::OpenFile(
                self.tree.root_path().join(&row.path),
            )),
        }
    }

    /// Rebuild the flattened row list. When `keep_path` is given the selection
    /// follows that entry; otherwise it stays on the same index, clamped.
    fn rebuild(&mut self, keep_path: Option<&Path>) {
        let mut rows = Vec::new();
        flatten(self.tree.root(), 0, &mut rows);
        self.rows = rows;

        if let Some(path) = keep_path
            && let Some(index) = self.rows.iter().position(|row| row.path == path)
        {
            self.selected = index;
        }
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }
}

/// Append `node`'s visible descendants to `out` in depth-first display order.
fn flatten(node: &FileNode, depth: usize, out: &mut Vec<FileRow>) {
    let Some(children) = node.children() else {
        return;
    };
    for child in children {
        out.push(FileRow {
            path: child.path.clone(),
            name: child.name.clone(),
            depth,
            kind: child.kind,
            expanded: child.expanded,
        });
        if child.kind == FileNodeKind::Directory && child.expanded {
            flatten(child, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_testkit::fixture_path;

    fn browser() -> FileBrowser {
        let tree = FileTree::load(fixture_path("file-tree")).expect("load file-tree fixture");
        FileBrowser::new(tree)
    }

    fn names(browser: &FileBrowser) -> Vec<&str> {
        browser.rows().iter().map(|row| row.name.as_str()).collect()
    }

    #[test]
    fn flattens_visible_root_entries_with_ignores_filtered() {
        let browser = browser();
        // Directories sort first, then files; `.git`, `node_modules`, `target`
        // are filtered by the default ignore list.
        assert_eq!(names(&browser), ["src", "tests", "README.md"]);
        assert_eq!(browser.selected_index(), 0);
        assert!(browser.rows().iter().all(|row| row.depth == 0));
    }

    #[test]
    fn move_down_and_up_saturate_at_the_bounds() {
        let mut browser = browser();
        browser.move_up();
        assert_eq!(browser.selected_index(), 0);
        browser.move_down();
        assert_eq!(browser.selected().unwrap().name, "tests");
        browser.move_down();
        assert_eq!(browser.selected().unwrap().name, "README.md");
        browser.move_down();
        assert_eq!(browser.selected().unwrap().name, "README.md");
    }

    #[test]
    fn activating_a_directory_expands_it_and_keeps_selection() {
        let mut browser = browser();
        assert_eq!(browser.activate().unwrap(), FileBrowserAction::Toggled);

        // `src` expands to reveal `nested/` (dir, sorted first) and `lib.rs`.
        assert_eq!(
            names(&browser),
            ["src", "nested", "lib.rs", "tests", "README.md"]
        );
        assert_eq!(browser.selected().unwrap().name, "src");
        assert!(browser.selected().unwrap().expanded);
        assert_eq!(browser.rows()[1].depth, 1);
    }

    #[test]
    fn activating_an_expanded_directory_collapses_it() {
        let mut browser = browser();
        browser.activate().unwrap();
        browser.activate().unwrap();

        assert_eq!(names(&browser), ["src", "tests", "README.md"]);
        assert!(!browser.selected().unwrap().expanded);
    }

    #[test]
    fn activating_a_file_reports_its_absolute_path() {
        let mut browser = browser();
        browser.activate().unwrap(); // expand src
        browser.move_down(); // nested
        browser.move_down(); // lib.rs

        let action = browser.activate().unwrap();
        match action {
            FileBrowserAction::OpenFile(path) => {
                assert!(path.is_absolute());
                assert!(path.ends_with("src/lib.rs"), "unexpected path: {path:?}");
            }
            other => panic!("expected OpenFile, got {other:?}"),
        }
    }
}
