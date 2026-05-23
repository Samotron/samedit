//! Completion popup view-model (spec §23 v0.4 / M4.3b).
//!
//! Pure state for manual LSP completions: rows, selected item, and optional
//! detail/docs text for the highlighted candidate.

/// One completion candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub insert_text: Option<String>,
}

impl CompletionItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: None,
            documentation: None,
            insert_text: None,
        }
    }

    pub fn insert_text(&self) -> &str {
        self.insert_text.as_deref().unwrap_or(&self.label)
    }
}

/// Headless completion popup state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionPopup {
    items: Vec<CompletionItem>,
    selection: usize,
}

impl CompletionPopup {
    pub fn new(items: Vec<CompletionItem>) -> Self {
        Self {
            items,
            selection: 0,
        }
    }

    pub fn items(&self) -> &[CompletionItem] {
        &self.items
    }

    pub fn selection(&self) -> usize {
        self.selection
    }

    pub fn highlighted(&self) -> Option<&CompletionItem> {
        self.items.get(self.selection)
    }

    pub fn move_down(&mut self) {
        if self.selection + 1 < self.items.len() {
            self.selection += 1;
        }
    }

    pub fn move_up(&mut self) {
        self.selection = self.selection.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_moves_within_bounds() {
        let mut popup = CompletionPopup::new(vec![
            CompletionItem::new("alpha"),
            CompletionItem::new("bravo"),
        ]);

        assert_eq!(popup.highlighted().unwrap().label, "alpha");
        popup.move_down();
        popup.move_down();
        assert_eq!(popup.selection(), 1);
        assert_eq!(popup.highlighted().unwrap().label, "bravo");
        popup.move_up();
        assert_eq!(popup.selection(), 0);
    }
}
