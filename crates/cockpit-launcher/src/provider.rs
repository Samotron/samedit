//! The provider trait — the one extension point of the launcher core.
//!
//! A provider turns a query into candidate [`Action`]s. The launcher
//! ([`crate::Launcher`]) owns ranking and merging, so providers stay dumb:
//! they decide *which* actions are relevant for a query and leave *how they
//! rank against other providers* to the launcher's matcher.

use crate::action::Action;

/// Default per-provider cap on rows in the merged result. Keeps one chatty
/// provider (a monorepo with hundreds of mise tasks) from drowning the list.
pub const DEFAULT_PROVIDER_QUOTA: usize = 5;

/// A source of launcher actions.
///
/// Implementors must be `Send + Sync` so the long-lived `cockpit-quick`
/// process can hold them behind a shared handle and re-query on every
/// keystroke.
pub trait ActionProvider: Send + Sync {
    /// Stable provider id (e.g. `mise`, `calculator`). Used for the per-row
    /// provider tag and deterministic tie-breaking.
    fn id(&self) -> &str;

    /// Candidate actions for `query`. The query is already trimmed. An empty
    /// query means "favourites" — return the provider's default entries (or
    /// nothing).
    fn search(&self, query: &str) -> Vec<Action>;

    /// Maximum rows this provider contributes to the merged list.
    fn quota(&self) -> usize {
        DEFAULT_PROVIDER_QUOTA
    }

    /// Whether the launcher should fuzzy-score this provider's titles against
    /// the query.
    ///
    /// `true` (default) for list providers like mise tasks — the launcher
    /// filters and ranks by fuzzy match. `false` for providers that already
    /// computed relevance themselves and emit one intentional action (the
    /// calculator, the URL opener): those are kept verbatim and float to the
    /// top, the way a Raycast calculator result does.
    fn fuzzy_filtered(&self) -> bool {
        true
    }
}
