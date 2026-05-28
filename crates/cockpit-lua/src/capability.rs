//! Capabilities — the default-deny gate for extension-visible side
//! effects. Extensions declare what they need; the user grants in
//! config (M9.4).
//!
//! v0.9 ships the declaration + grant machinery, which is enough to
//! refuse undeclared capability calls at the API boundary. The
//! corresponding `fs.read.project` / `process` / `clipboard.*` APIs
//! land alongside their dedicated namespaces — until then, the
//! capability set is *carried* but doesn't unlock any new Lua surface.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One capability token. The set is intentionally tiny — adding to it
/// is a plan change (M9.4), not a runtime detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// `fs.read.project` — read files inside the project root.
    FsReadProject,
    /// `process` — spawn declared commands via the `ProcessRunner` seam.
    Process,
    /// `clipboard.read` — read the OS clipboard.
    ClipboardRead,
    /// `clipboard.write` — write to the OS clipboard.
    ClipboardWrite,
}

impl Capability {
    /// Spec-stable token name.
    pub fn token(self) -> &'static str {
        match self {
            Self::FsReadProject => "fs.read.project",
            Self::Process => "process",
            Self::ClipboardRead => "clipboard.read",
            Self::ClipboardWrite => "clipboard.write",
        }
    }

    /// Parse a token name. Returns `None` for unknown tokens — caller
    /// raises a [`CapabilityError`].
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "fs.read.project" => Some(Self::FsReadProject),
            "process" => Some(Self::Process),
            "clipboard.read" => Some(Self::ClipboardRead),
            "clipboard.write" => Some(Self::ClipboardWrite),
            _ => None,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

/// Capability bundle for an extension.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilitySet {
    tokens: BTreeSet<Capability>,
}

impl CapabilitySet {
    /// Empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from an iterator of tokens.
    pub fn from_tokens<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        Self {
            tokens: iter.into_iter().collect(),
        }
    }

    /// True if `cap` is in the set.
    pub fn contains(&self, cap: &Capability) -> bool {
        self.tokens.contains(cap)
    }

    /// Add a capability.
    pub fn insert(&mut self, cap: Capability) {
        self.tokens.insert(cap);
    }

    /// Intersection with another set. Used to compute "effective" caps
    /// (declared ∩ granted).
    pub fn intersect(&self, other: &CapabilitySet) -> CapabilitySet {
        Self {
            tokens: self.tokens.intersection(&other.tokens).copied().collect(),
        }
    }

    /// Iterate capabilities in stable order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.tokens.iter().copied()
    }

    /// Number of capabilities in the set.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// True when no capabilities are granted.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }
}

/// Errors raised while parsing capability declarations.
#[derive(Debug, Error)]
pub enum CapabilityError {
    /// The `@cockpit:requires …` header listed an unknown token.
    #[error("unknown capability token `{0}`")]
    Unknown(String),
}

/// Parse the `--[[ @cockpit:requires X, Y ]]--` header from `source`.
/// Returns an empty set when no header is present. Tokens are
/// comma-or-whitespace separated.
pub fn parse_requires_header(source: &str) -> Result<CapabilitySet, CapabilityError> {
    let mut set = CapabilitySet::new();
    let needle = "@cockpit:requires";
    let Some(idx) = source.find(needle) else {
        return Ok(set);
    };
    let after = &source[idx + needle.len()..];
    // Pick up everything until the next bracket or newline.
    let end = after.find([']', '\n']).unwrap_or(after.len());
    let payload = &after[..end];
    for token in payload.split(|c: char| c == ',' || c.is_whitespace()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let cap = Capability::parse(token).ok_or_else(|| CapabilityError::Unknown(token.into()))?;
        set.insert(cap);
    }
    Ok(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_header_yields_empty_set() {
        let set = parse_requires_header("print('hi')\n").unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn parses_single_token_header() {
        let set = parse_requires_header("--[[ @cockpit:requires process ]]--\n").unwrap();
        assert!(set.contains(&Capability::Process));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn parses_multi_token_header() {
        let set = parse_requires_header(
            "--[[ @cockpit:requires fs.read.project, process, clipboard.write ]]--\n",
        )
        .unwrap();
        assert!(set.contains(&Capability::FsReadProject));
        assert!(set.contains(&Capability::Process));
        assert!(set.contains(&Capability::ClipboardWrite));
        assert!(!set.contains(&Capability::ClipboardRead));
    }

    #[test]
    fn unknown_token_is_error() {
        let err = parse_requires_header("--[[ @cockpit:requires telepathy ]]--").unwrap_err();
        assert!(matches!(err, CapabilityError::Unknown(t) if t == "telepathy"));
    }

    #[test]
    fn intersection_returns_overlap() {
        let a = CapabilitySet::from_tokens([Capability::Process, Capability::ClipboardRead]);
        let b = CapabilitySet::from_tokens([Capability::Process, Capability::ClipboardWrite]);
        let both = a.intersect(&b);
        assert!(both.contains(&Capability::Process));
        assert!(!both.contains(&Capability::ClipboardRead));
        assert!(!both.contains(&Capability::ClipboardWrite));
    }
}
