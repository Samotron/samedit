//! Minimal yes/no confirmation prompt.
//!
//! Used by the format-on-save detection flow (M4.4) to surface "Add `format`
//! task to `mise.toml`?" before writing anything (AGENTS.md rule #6: detect,
//! surface, prompt — never silently modify). Pure view-model: state changes
//! are unit-testable without a window.

/// A modal yes/no question awaiting a user decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmPrompt {
    title: String,
    body: String,
    /// True when the highlighted choice is "Yes". Defaults to false so an
    /// accidental Enter never modifies project state.
    selection: bool,
}

impl ConfirmPrompt {
    /// New prompt with a short title and a longer body, defaulting to "No".
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            selection: false,
        }
    }

    /// Heading shown in bold above the body text.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Multi-line description explaining what cockpit will do on confirm.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Currently highlighted choice — `true` for "Yes", `false` for "No".
    pub fn selection(&self) -> bool {
        self.selection
    }

    /// Move the highlight to "Yes".
    pub fn highlight_yes(&mut self) {
        self.selection = true;
    }

    /// Move the highlight to "No".
    pub fn highlight_no(&mut self) {
        self.selection = false;
    }

    /// Flip the highlight between Yes and No.
    pub fn toggle(&mut self) {
        self.selection = !self.selection;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_selection_is_no() {
        let prompt = ConfirmPrompt::new("Add format task?", "Cockpit will append [tasks.format].");
        assert!(!prompt.selection(), "default must be safe");
    }

    #[test]
    fn highlight_helpers_change_selection() {
        let mut prompt = ConfirmPrompt::new("title", "body");
        prompt.highlight_yes();
        assert!(prompt.selection());
        prompt.highlight_no();
        assert!(!prompt.selection());
        prompt.toggle();
        assert!(prompt.selection());
        prompt.toggle();
        assert!(!prompt.selection());
    }

    #[test]
    fn title_and_body_round_trip() {
        let prompt = ConfirmPrompt::new("Confirm", "Long\nmulti-line\nbody.");
        assert_eq!(prompt.title(), "Confirm");
        assert_eq!(prompt.body(), "Long\nmulti-line\nbody.");
    }
}
