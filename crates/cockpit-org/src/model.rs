//! Org domain model: headings and files.
//!
//! These are owned, pure-data types. Round-trip fidelity is *not* a property of
//! re-serialising these structs — we never re-emit them. The authoritative
//! bytes live in [`OrgFile::source`]; edits happen via line-range replacement
//! (see [`crate::edit`]). The structs are the *index* over that source.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;

use crate::timestamp::Timestamp;

/// A single Org headline plus its section body and nested children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Heading {
    /// Headline depth (number of leading `*`).
    pub level: usize,
    /// Title text with the stars, keyword, priority cookie, and tags removed.
    pub title: String,
    /// TODO keyword, if the headline carries one.
    pub todo_keyword: Option<String>,
    /// Priority cookie char (`[#A]` → `'A'`), if present.
    pub priority: Option<char>,
    /// Tags (`:work:urgent:` → `["work", "urgent"]`).
    pub tags: Vec<String>,
    /// `SCHEDULED:` timestamp, if present.
    pub scheduled: Option<Timestamp>,
    /// `DEADLINE:` timestamp, if present.
    pub deadline: Option<Timestamp>,
    /// `CLOSED:` timestamp, if present.
    pub closed: Option<Timestamp>,
    /// Section body text (lines after the headline and planning line, before
    /// the first child). Informational only — never used to reconstruct bytes.
    pub body: String,
    /// Half-open line range `[start, end)` of this heading's *own* extent
    /// (headline + planning + body), excluding child subtrees. 0-based.
    pub line_range: Range<usize>,
    /// Nested child headings, in document order.
    pub children: Vec<Heading>,
}

impl Heading {
    /// `true` if this heading carries any TODO keyword.
    pub fn has_keyword(&self) -> bool {
        self.todo_keyword.is_some()
    }

    /// `true` if this heading is tagged with `tag` (case-sensitive, as Org is).
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t == tag)
    }

    /// The half-open line range covering this heading **and all descendants**.
    pub fn subtree_line_range(&self) -> Range<usize> {
        let end = self
            .children
            .last()
            .map(|c| c.subtree_line_range().end)
            .unwrap_or(self.line_range.end);
        self.line_range.start..end
    }

    /// Pre-order iterator over this heading and every descendant.
    pub fn iter(&self) -> PreorderIter<'_> {
        PreorderIter { stack: vec![self] }
    }
}

/// Pre-order traversal over a heading subtree.
pub struct PreorderIter<'a> {
    stack: Vec<&'a Heading>,
}

impl<'a> Iterator for PreorderIter<'a> {
    type Item = &'a Heading;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        // Push children in reverse so the first child is visited next.
        self.stack.extend(node.children.iter().rev());
        Some(node)
    }
}

/// A parsed `.org` file: its authoritative source plus the heading index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgFile {
    /// Path the file was loaded from.
    pub path: PathBuf,
    /// Authoritative source bytes. All edits replace line ranges within this.
    pub source: String,
    /// Stable content hash for on-disk-change detection.
    pub content_hash: u64,
    /// Top-level headings (lowest level seen), with children nested beneath.
    pub headings: Vec<Heading>,
}

impl OrgFile {
    /// Pre-order iterator over every heading in the file.
    pub fn iter_headings(&self) -> impl Iterator<Item = &Heading> {
        self.headings.iter().flat_map(Heading::iter)
    }

    /// Compute the stable content hash for a source string.
    pub fn hash_source(source: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut hasher);
        hasher.finish()
    }

    /// `true` if `on_disk` differs from this file's recorded content hash.
    pub fn changed_on_disk(&self, on_disk: &str) -> bool {
        Self::hash_source(on_disk) != self.content_hash
    }

    /// The file's source split into lines (without terminators).
    pub fn line(&self, idx: usize) -> Option<&str> {
        self.source.lines().nth(idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(level: usize, title: &str, range: Range<usize>) -> Heading {
        Heading {
            level,
            title: title.to_string(),
            line_range: range,
            ..Default::default()
        }
    }

    #[test]
    fn preorder_visits_parent_then_children() {
        let mut root = leaf(1, "root", 0..1);
        let mut child = leaf(2, "child", 1..2);
        child.children.push(leaf(3, "grandchild", 2..3));
        root.children.push(child);
        root.children.push(leaf(2, "sibling", 3..4));

        let titles: Vec<_> = root.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["root", "child", "grandchild", "sibling"]);
    }

    #[test]
    fn subtree_range_spans_descendants() {
        let mut root = leaf(1, "root", 0..1);
        let mut child = leaf(2, "child", 1..2);
        child.children.push(leaf(3, "grandchild", 2..5));
        root.children.push(child);
        assert_eq!(root.subtree_line_range(), 0..5);
        assert_eq!(root.line_range, 0..1);
    }

    #[test]
    fn hash_detects_change() {
        let h = OrgFile::hash_source("* a\n");
        assert_eq!(h, OrgFile::hash_source("* a\n"));
        assert_ne!(h, OrgFile::hash_source("* b\n"));
    }
}
