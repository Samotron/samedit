//! Command palette view-model (spec §16 / §23 v0.1 M1.17).
//!
//! Headless model of the `Ctrl+Shift+P` palette: an ordered list of
//! [`PaletteEntry`] items (snapshotted from the [`Registry`]), a query string,
//! a filtered & ranked match list, and selection. Activating the highlighted
//! entry returns the [`CommandId`] for the caller to dispatch through the
//! single command spine.
//!
//! Filtering is a deterministic subsequence match with simple scoring:
//! prefix > word-start > generic subsequence. Good enough for v0.1; spec §16
//! marks "command palette filtering" as a golden-test target so the behaviour
//! is locked in by tests.
//!
//! [`Registry`]: cockpit_commands::Registry

use cockpit_commands::{CommandId, Registry};

/// One row in the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub id: CommandId,
    pub title: String,
}

impl PaletteEntry {
    /// Construct a palette entry.
    pub fn new(id: impl Into<CommandId>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
        }
    }
}

/// One filtered match (entry index + score, higher is better).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteMatch {
    pub entry_index: usize,
    pub score: u32,
}

/// Command palette view-model.
#[derive(Debug, Clone)]
pub struct Palette {
    entries: Vec<PaletteEntry>,
    query: String,
    matches: Vec<PaletteMatch>,
    selection: usize,
}

impl Palette {
    /// Build a palette from explicit entries.
    pub fn new(entries: Vec<PaletteEntry>) -> Self {
        let mut palette = Self {
            entries,
            query: String::new(),
            matches: Vec::new(),
            selection: 0,
        };
        palette.recompute();
        palette
    }

    /// Snapshot every command in the registry, in id order.
    pub fn from_registry(registry: &Registry) -> Self {
        let entries = registry
            .commands()
            .map(|c| PaletteEntry::new(c.id().clone(), c.title().to_string()))
            .collect();
        Self::new(entries)
    }

    /// All known palette entries, in display order.
    pub fn entries(&self) -> &[PaletteEntry] {
        &self.entries
    }

    /// Current query string.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Filtered matches in ranked order.
    pub fn matches(&self) -> &[PaletteMatch] {
        &self.matches
    }

    /// Index into [`matches`](Self::matches) of the highlighted row.
    pub fn selection(&self) -> usize {
        self.selection
    }

    /// Replace the query and re-rank matches.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.recompute();
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

    /// Currently-highlighted entry, if any.
    pub fn highlighted(&self) -> Option<&PaletteEntry> {
        let m = self.matches.get(self.selection)?;
        self.entries.get(m.entry_index)
    }

    /// Activate the highlighted entry and return its command id.
    pub fn activate(&self) -> Option<CommandId> {
        self.highlighted().map(|e| e.id.clone())
    }

    fn recompute(&mut self) {
        let query = self.query.trim();
        self.matches.clear();

        if query.is_empty() {
            self.matches
                .extend((0..self.entries.len()).map(|i| PaletteMatch {
                    entry_index: i,
                    score: 0,
                }));
        } else {
            for (i, entry) in self.entries.iter().enumerate() {
                if let Some(score) = score_match(&entry.title, query) {
                    self.matches.push(PaletteMatch {
                        entry_index: i,
                        score,
                    });
                }
            }
            // Stable sort: higher score first; ties keep registry order.
            self.matches.sort_by_key(|m| std::cmp::Reverse(m.score));
        }

        if self.selection >= self.matches.len() {
            self.selection = 0;
        }
    }
}

/// Score `query` against `title`. Returns `None` if no subsequence match.
///
/// Scoring: prefix match = 300, word-start match = 200, subsequence = 100,
/// minus the position of the first match character (closer is better).
fn score_match(title: &str, query: &str) -> Option<u32> {
    let title_lc = title.to_ascii_lowercase();
    let query_lc = query.to_ascii_lowercase();

    if title_lc.starts_with(&query_lc) {
        return Some(300);
    }

    // Word-start: every query char must appear at the start of a word.
    if word_start_match(&title_lc, &query_lc) {
        return Some(200);
    }

    let first = subsequence_first_index(&title_lc, &query_lc)?;
    Some(100u32.saturating_sub(first.min(99) as u32))
}

fn word_start_match(title: &str, query: &str) -> bool {
    let mut q = query.chars();
    let Some(mut next) = q.next() else {
        return true;
    };
    let mut at_word_start = true;
    for ch in title.chars() {
        if at_word_start && ch == next {
            match q.next() {
                Some(c) => next = c,
                None => return true,
            }
        }
        at_word_start = !ch.is_alphanumeric();
    }
    false
}

fn subsequence_first_index(title: &str, query: &str) -> Option<usize> {
    let mut q = query.chars().peekable();
    let Some(&first) = q.peek() else {
        return Some(0);
    };
    let mut first_index = None;
    for (i, ch) in title.char_indices() {
        if Some(&ch) == q.peek() {
            if first_index.is_none() {
                first_index = Some(i);
            }
            q.next();
            if q.peek().is_none() {
                return first_index;
            }
            let _ = first;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cockpit_commands::{Command, Registry};

    fn fixture() -> Palette {
        Palette::new(vec![
            PaletteEntry::new("file.save", "File: Save"),
            PaletteEntry::new("file.open", "File: Open"),
            PaletteEntry::new("project.open", "Project: Open Project"),
            PaletteEntry::new("mise.run_task", "Mise: Run Task"),
            PaletteEntry::new("terminal.focus", "Terminal: Focus"),
        ])
    }

    #[test]
    fn empty_query_lists_all_entries_in_order() {
        let palette = fixture();
        let titles: Vec<_> = palette
            .matches()
            .iter()
            .map(|m| palette.entries()[m.entry_index].title.as_str())
            .collect();
        assert_eq!(
            titles,
            vec![
                "File: Save",
                "File: Open",
                "Project: Open Project",
                "Mise: Run Task",
                "Terminal: Focus",
            ]
        );
        assert_eq!(palette.selection(), 0);
    }

    #[test]
    fn prefix_match_outranks_subsequence_match() {
        let mut palette = fixture();
        palette.set_query("file");

        let top = palette.highlighted().unwrap();
        assert_eq!(top.id, CommandId::from("file.save"));
        // Both "File: Save" and "File: Open" match by prefix; both retained.
        assert_eq!(palette.matches().len(), 2);
    }

    #[test]
    fn word_start_match_finds_acronyms() {
        let mut palette = fixture();
        palette.set_query("rt");
        let top = palette.highlighted().unwrap();
        assert_eq!(top.id, CommandId::from("mise.run_task"));
    }

    #[test]
    fn no_match_clears_results() {
        let mut palette = fixture();
        palette.set_query("zzznope");
        assert!(palette.matches().is_empty());
        assert!(palette.activate().is_none());
    }

    #[test]
    fn typing_filters_then_backspace_restores() {
        let mut palette = fixture();
        palette.push_char('m');
        assert_eq!(
            palette.highlighted().unwrap().id,
            CommandId::from("mise.run_task")
        );
        palette.pop_char();
        assert_eq!(palette.matches().len(), palette.entries().len());
    }

    #[test]
    fn selection_navigation_saturates_at_bounds() {
        let mut palette = fixture();
        palette.move_up();
        assert_eq!(palette.selection(), 0);

        for _ in 0..50 {
            palette.move_down();
        }
        assert_eq!(palette.selection(), palette.matches().len() - 1);
    }

    #[test]
    fn changing_query_resets_selection_when_out_of_range() {
        let mut palette = fixture();
        palette.move_down();
        palette.move_down();
        assert_eq!(palette.selection(), 2);

        palette.set_query("save");
        assert_eq!(palette.selection(), 0);
        assert_eq!(palette.matches().len(), 1);
    }

    #[test]
    fn from_registry_snapshots_in_id_order() {
        let mut registry = Registry::new();
        registry
            .register(Command::new("z.last", "Z: Last", |_| {}))
            .unwrap();
        registry
            .register(Command::new("a.first", "A: First", |_| {}))
            .unwrap();

        let palette = Palette::from_registry(&registry);
        let ids: Vec<_> = palette
            .entries()
            .iter()
            .map(|e| e.id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec!["a.first", "z.last"]);
    }

    #[test]
    fn activate_returns_command_id_of_highlighted_entry() {
        let mut palette = fixture();
        palette.set_query("term");
        assert_eq!(palette.activate(), Some(CommandId::from("terminal.focus")));
    }
}
