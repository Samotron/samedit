//! Fuzzy file-open view-model (spec §23 v0.2 / M2.1).
//!
//! A headless model of the `Ctrl+P` finder: a candidate list of project file
//! paths, a query string, a ranked match list (scored by `nucleo-matcher`),
//! and a selection. It is a pure data structure — the binary builds the
//! candidate list from [`walk_project_files`](cockpit_project::walk_project_files)
//! and dispatches the highlighted path.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// One ranked fuzzy match: an index into the candidate list plus its score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuzzyMatch {
    pub item_index: usize,
    pub score: u32,
}

/// Fuzzy file finder view-model. Deterministic and headless.
#[derive(Debug, Clone)]
pub struct FuzzyFinder {
    items: Vec<String>,
    query: String,
    matches: Vec<FuzzyMatch>,
    selection: usize,
}

impl FuzzyFinder {
    /// Build a finder over candidate paths (project-relative path strings).
    pub fn new(items: Vec<String>) -> Self {
        let mut finder = Self {
            items,
            query: String::new(),
            matches: Vec::new(),
            selection: 0,
        };
        finder.recompute();
        finder
    }

    /// All candidate paths.
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Ranked matches, best first.
    pub fn matches(&self) -> &[FuzzyMatch] {
        &self.matches
    }

    /// Index into [`matches`](Self::matches) of the highlighted row.
    pub fn selection(&self) -> usize {
        self.selection
    }

    /// True when nothing matches the current query.
    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    /// Append one character to the query.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.recompute();
    }

    /// Remove the last character of the query.
    pub fn pop_char(&mut self) {
        self.query.pop();
        self.recompute();
    }

    /// Replace the query and re-rank.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.recompute();
    }

    /// Move the selection down one match, saturating at the end.
    pub fn move_down(&mut self) {
        if self.selection + 1 < self.matches.len() {
            self.selection += 1;
        }
    }

    /// Move the selection up one match, saturating at the top.
    pub fn move_up(&mut self) {
        self.selection = self.selection.saturating_sub(1);
    }

    /// The highlighted candidate path, if any.
    pub fn highlighted(&self) -> Option<&str> {
        let matched = self.matches.get(self.selection)?;
        self.items.get(matched.item_index).map(String::as_str)
    }

    fn recompute(&mut self) {
        self.matches.clear();
        let query = self.query.trim();

        if query.is_empty() {
            // Empty query: every candidate, in the list's (sorted) order.
            self.matches = (0..self.items.len())
                .map(|item_index| FuzzyMatch {
                    item_index,
                    score: 0,
                })
                .collect();
        } else {
            let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
            let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
            let mut buffer = Vec::new();
            for (item_index, item) in self.items.iter().enumerate() {
                let haystack = Utf32Str::new(item, &mut buffer);
                if let Some(score) = pattern.score(haystack, &mut matcher) {
                    self.matches.push(FuzzyMatch { item_index, score });
                }
            }
            // Best score first; ties broken by shorter, then lexicographic
            // path so ranking is deterministic.
            self.matches.sort_by(|a, b| {
                b.score
                    .cmp(&a.score)
                    .then_with(|| {
                        self.items[a.item_index]
                            .len()
                            .cmp(&self.items[b.item_index].len())
                    })
                    .then_with(|| self.items[a.item_index].cmp(&self.items[b.item_index]))
            });
        }

        if self.selection >= self.matches.len() {
            self.selection = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finder() -> FuzzyFinder {
        FuzzyFinder::new(vec![
            "src/main.rs".to_string(),
            "src/app.rs".to_string(),
            "src/editor/buffer.rs".to_string(),
            "tests/golden_editor.rs".to_string(),
            "README.md".to_string(),
        ])
    }

    #[test]
    fn empty_query_lists_every_candidate_in_order() {
        let finder = finder();
        assert_eq!(finder.matches().len(), 5);
        assert_eq!(finder.highlighted(), Some("src/main.rs"));
    }

    #[test]
    fn fuzzy_query_filters_and_ranks() {
        let mut finder = finder();
        finder.set_query("buf");
        assert_eq!(finder.highlighted(), Some("src/editor/buffer.rs"));
    }

    #[test]
    fn subsequence_match_spans_path_separators() {
        let mut finder = finder();
        finder.set_query("smain");
        assert_eq!(finder.highlighted(), Some("src/main.rs"));
    }

    #[test]
    fn no_match_clears_results() {
        let mut finder = finder();
        finder.set_query("zzzznope");
        assert!(finder.is_empty());
        assert_eq!(finder.highlighted(), None);
    }

    #[test]
    fn typing_then_backspace_restores_full_list() {
        let mut finder = finder();
        finder.push_char('x');
        finder.push_char('y');
        finder.push_char('z');
        assert!(finder.is_empty());
        finder.pop_char();
        finder.pop_char();
        finder.pop_char();
        assert_eq!(finder.matches().len(), 5);
    }

    #[test]
    fn selection_navigation_saturates_at_bounds() {
        let mut finder = finder();
        finder.move_up();
        assert_eq!(finder.selection(), 0);
        for _ in 0..50 {
            finder.move_down();
        }
        assert_eq!(finder.selection(), finder.matches().len() - 1);
    }
}
