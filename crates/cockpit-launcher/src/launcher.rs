//! The launcher core: register providers, query them, rank and merge the
//! results into one deterministic list.
//!
//! Ranking reuses `nucleo-matcher` — the same matcher the in-cockpit file
//! finder and palette use (`cockpit_ui::file_finder`), so ranking-quality
//! regressions surface in one place. The merge is two-stage: each provider's
//! actions are scored and capped to its [`quota`](ActionProvider::quota),
//! then the survivors are merged and sorted by score so no single provider
//! can drown the list.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::action::Action;
use crate::provider::ActionProvider;

/// Default cap on rows in the merged list (mirrors `launcher.ui.max_rows`).
pub const DEFAULT_MAX_ROWS: usize = 8;

/// Base score for verbatim (non-fuzzy) provider actions, so a computed
/// result — a calculator answer, an "open URL" — outranks fuzzy matches.
/// Well above any realistic `nucleo` title score.
const VERBATIM_BASE_SCORE: u32 = 1_000_000;

/// A ranked, provider-tagged action ready to display or dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedAction {
    /// The action itself.
    pub action: Action,
    /// Id of the provider that emitted it.
    pub provider: String,
    /// Merge score; higher ranks first.
    pub score: u32,
}

/// The headless launcher. Holds providers and merges their results.
#[derive(Default)]
pub struct Launcher {
    providers: Vec<Box<dyn ActionProvider>>,
    max_rows: usize,
}

impl Launcher {
    /// New launcher with the default row cap and no providers.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            max_rows: DEFAULT_MAX_ROWS,
        }
    }

    /// Set the merged-list row cap (`launcher.ui.max_rows`).
    pub fn with_max_rows(mut self, max_rows: usize) -> Self {
        self.max_rows = max_rows;
        self
    }

    /// Register a provider. Query order follows registration order, which is
    /// the final tie-break when scores are equal.
    pub fn register(&mut self, provider: Box<dyn ActionProvider>) -> &mut Self {
        self.providers.push(provider);
        self
    }

    /// Registered provider ids, in registration order.
    pub fn provider_ids(&self) -> impl Iterator<Item = &str> {
        self.providers.iter().map(|p| p.id())
    }

    /// Rank and merge every provider's actions for `query`.
    pub fn search(&self, query: &str) -> Vec<RankedAction> {
        let query = query.trim();
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = (!query.is_empty())
            .then(|| Pattern::parse(query, CaseMatching::Smart, Normalization::Smart));

        let mut merged: Vec<RankedAction> = Vec::new();
        for provider in &self.providers {
            let actions = provider.search(query);
            let mut scored: Vec<RankedAction> = if provider.fuzzy_filtered() {
                score_fuzzy(&mut matcher, pattern.as_ref(), provider.id(), actions)
            } else {
                score_verbatim(provider.id(), actions)
            };

            sort_ranked(&mut scored);
            scored.truncate(provider.quota());
            merged.extend(scored);
        }

        // Stable cross-provider merge. Equal scores fall back to title then
        // id, both provider-independent, so the result is fully deterministic.
        sort_ranked(&mut merged);
        merged.truncate(self.max_rows);
        merged
    }
}

/// Score a provider's actions by fuzzy-matching the query against each
/// title. An empty query (no pattern) keeps every action at score 0, giving a
/// stable "favourites" listing.
fn score_fuzzy(
    matcher: &mut Matcher,
    pattern: Option<&Pattern>,
    provider_id: &str,
    actions: Vec<Action>,
) -> Vec<RankedAction> {
    let mut buffer = Vec::new();
    let mut out = Vec::new();
    for action in actions {
        let score = match pattern {
            None => Some(0),
            Some(pattern) => {
                let haystack = Utf32Str::new(action.haystack(), &mut buffer);
                pattern.score(haystack, matcher)
            }
        };
        if let Some(score) = score {
            out.push(RankedAction {
                action,
                provider: provider_id.to_string(),
                score,
            });
        }
    }
    out
}

/// Keep a provider's actions verbatim, assigning descending scores from a
/// high base so they outrank fuzzy matches and preserve emission order.
fn score_verbatim(provider_id: &str, actions: Vec<Action>) -> Vec<RankedAction> {
    actions
        .into_iter()
        .enumerate()
        .map(|(i, action)| RankedAction {
            action,
            provider: provider_id.to_string(),
            score: VERBATIM_BASE_SCORE.saturating_sub(i as u32),
        })
        .collect()
}

/// Order scored actions: score desc, then title, then id — deterministic
/// regardless of provider iteration order.
fn sort_ranked(scored: &mut [RankedAction]) {
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.action.title.cmp(&b.action.title))
            .then_with(|| a.action.id.cmp(&b.action.id))
    });
}
