//! TODO-keyword workflow.
//!
//! The v0.12 default workflow is `TODO | DONE`. Custom multi-state workflows
//! (`TODO | NEXT | WAIT | DONE`) are a v0.12.x follow-up, but the type here is
//! already general enough to carry them — it only ever splits a flat keyword
//! sequence into the "active" and "done" buckets using Org's rule.

/// A TODO-keyword workflow: a set of "active" keywords and a set of "done"
/// keywords.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keywords {
    /// Keywords that mark an open task (before the `|`).
    pub todo: Vec<String>,
    /// Keywords that mark a completed task (after the `|`).
    pub done: Vec<String>,
}

impl Default for Keywords {
    fn default() -> Self {
        Keywords {
            todo: vec!["TODO".to_string()],
            done: vec!["DONE".to_string()],
        }
    }
}

impl Keywords {
    /// Build a workflow from a flat keyword sequence, applying Org's rule for
    /// sequences without an explicit `|`: the **last** keyword is the done
    /// state, all earlier ones are active.
    ///
    /// `["TODO", "DONE"]` → todo `[TODO]`, done `[DONE]`.
    /// `["TODO", "NEXT", "DONE"]` → todo `[TODO, NEXT]`, done `[DONE]`.
    /// A single keyword is treated as active with no done state.
    pub fn from_sequence<I, S>(seq: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut all: Vec<String> = seq.into_iter().map(Into::into).collect();
        if all.len() <= 1 {
            return Keywords {
                todo: all,
                done: Vec::new(),
            };
        }
        let done = all.split_off(all.len() - 1);
        Keywords { todo: all, done }
    }

    /// Every keyword, active then done — the set a headline line may begin with.
    pub fn all(&self) -> impl Iterator<Item = &str> {
        self.todo.iter().chain(self.done.iter()).map(String::as_str)
    }

    /// `true` if `word` is any known keyword.
    pub fn contains(&self, word: &str) -> bool {
        self.all().any(|k| k == word)
    }

    /// `true` if `word` is a "done" keyword.
    pub fn is_done(&self, word: &str) -> bool {
        self.done.iter().any(|k| k == word)
    }

    /// The orgize `(todo, done)` pair this workflow maps to.
    pub fn as_orgize(&self) -> (Vec<String>, Vec<String>) {
        (self.todo.clone(), self.done.clone())
    }

    /// Cycle a keyword to the next state in the workflow.
    ///
    /// Order is active keywords, then done keywords, then back to no keyword:
    /// `None → first todo → … → last done → None`.
    pub fn cycle(&self, current: Option<&str>) -> Option<String> {
        let order: Vec<&str> = self.all().collect();
        match current {
            None => order.first().map(|s| s.to_string()),
            Some(cur) => match order.iter().position(|k| *k == cur) {
                Some(i) if i + 1 < order.len() => Some(order[i + 1].to_string()),
                // Last keyword (or unknown) cycles back to no keyword.
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_workflow() {
        let kw = Keywords::default();
        assert_eq!(kw.todo, ["TODO"]);
        assert_eq!(kw.done, ["DONE"]);
    }

    #[test]
    fn last_keyword_is_done() {
        let kw = Keywords::from_sequence(["TODO", "NEXT", "DONE"]);
        assert_eq!(kw.todo, ["TODO", "NEXT"]);
        assert_eq!(kw.done, ["DONE"]);
        assert!(kw.is_done("DONE"));
        assert!(!kw.is_done("NEXT"));
    }

    #[test]
    fn cycle_default() {
        let kw = Keywords::default();
        assert_eq!(kw.cycle(None).as_deref(), Some("TODO"));
        assert_eq!(kw.cycle(Some("TODO")).as_deref(), Some("DONE"));
        assert_eq!(kw.cycle(Some("DONE")), None);
    }

    #[test]
    fn cycle_unknown_resets() {
        let kw = Keywords::default();
        assert_eq!(kw.cycle(Some("WAIT")), None);
    }
}
