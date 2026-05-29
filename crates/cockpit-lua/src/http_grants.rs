//! Per-collection grant store for the `http.scripts` capability
//! (v0.11 M11.6).
//!
//! Users grant the capability per-collection in
//! `~/.config/cockpit/extensions.toml`:
//!
//! ```toml
//! [http]
//! granted_collections = [
//!   "/home/alice/api-tests",
//!   "/home/alice/billing",
//! ]
//! ```
//!
//! `is_granted` walks parent directories so granting the parent
//! covers nested collections — useful for monorepos where each
//! service ships a `cockpit-http/` directory under one repo root.
//!
//! Default-deny is the entire point: a missing file, an empty
//! `granted_collections`, or a typo in the path all keep scripts
//! disabled. The TOML parser is the existing `cockpit-config`
//! dependency-tree's `toml` crate; we don't introduce a new one.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Loaded grant set. Empty by default; the binary populates it from
/// `extensions.toml` at startup and on `Debug: Reload Config`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HttpScriptsGrants {
    granted_roots: Vec<PathBuf>,
}

impl HttpScriptsGrants {
    /// Build from an explicit list of canonical roots. Public so the
    /// binary can populate from any source (CLI flag, env var, …),
    /// not just the TOML file.
    pub fn from_roots(roots: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            granted_roots: roots.into_iter().collect(),
        }
    }

    /// True when `collection_root` (or any ancestor) is in the grant
    /// list. Path comparison is a prefix walk on path components so
    /// `/home/alice` grants `/home/alice/api/v1` without needing the
    /// user to list the leaf directly.
    pub fn is_granted(&self, collection_root: &Path) -> bool {
        self.granted_roots
            .iter()
            .any(|root| starts_with_or_equal(collection_root, root))
    }

    /// Number of distinct roots currently granted. Useful for
    /// `Debug: Show Extensions` to tell the user how many collections
    /// can run scripts.
    pub fn len(&self) -> usize {
        self.granted_roots.len()
    }

    /// True when the grant list is empty.
    pub fn is_empty(&self) -> bool {
        self.granted_roots.is_empty()
    }
}

/// Parse an `extensions.toml` source string. Unknown keys outside
/// `[http]` are tolerated — other cockpit subsystems share the same
/// file, so the parser must not reject foreign sections.
pub fn parse_extensions_toml(source: &str) -> Result<HttpScriptsGrants, GrantsError> {
    let parsed: ExtensionsFile = toml::from_str(source).map_err(GrantsError::Toml)?;
    let roots: Vec<PathBuf> = parsed
        .http
        .map(|http| http.granted_collections)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect();
    Ok(HttpScriptsGrants::from_roots(roots))
}

#[derive(Debug, Deserialize)]
struct ExtensionsFile {
    #[serde(default)]
    http: Option<HttpSection>,
}

#[derive(Debug, Deserialize)]
struct HttpSection {
    #[serde(default)]
    granted_collections: Vec<String>,
}

fn starts_with_or_equal(candidate: &Path, root: &Path) -> bool {
    candidate
        .components()
        .zip(root.components())
        .all(|(a, b)| a == b)
        && candidate.components().count() >= root.components().count()
}

/// Failure modes for [`parse_extensions_toml`].
#[derive(Debug, Error)]
pub enum GrantsError {
    #[error("malformed extensions.toml: {0}")]
    Toml(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_grants_deny_everything() {
        let grants = HttpScriptsGrants::default();
        assert!(!grants.is_granted(Path::new("/tmp/anything")));
        assert!(grants.is_empty());
    }

    #[test]
    fn explicit_root_grants_itself() {
        let grants = HttpScriptsGrants::from_roots([PathBuf::from("/home/alice/api")]);
        assert!(grants.is_granted(Path::new("/home/alice/api")));
    }

    #[test]
    fn explicit_root_grants_nested_collection() {
        let grants = HttpScriptsGrants::from_roots([PathBuf::from("/home/alice")]);
        assert!(grants.is_granted(Path::new("/home/alice/api/v1")));
    }

    #[test]
    fn unrelated_path_is_denied() {
        let grants = HttpScriptsGrants::from_roots([PathBuf::from("/home/alice/api")]);
        assert!(!grants.is_granted(Path::new("/home/bob/api")));
    }

    #[test]
    fn parses_a_typical_extensions_toml() {
        let source = r#"
[http]
granted_collections = ["/home/alice/api", "/srv/billing"]
"#;
        let grants = parse_extensions_toml(source).unwrap();
        assert_eq!(grants.len(), 2);
        assert!(grants.is_granted(Path::new("/home/alice/api/users")));
    }

    #[test]
    fn missing_http_section_yields_empty_grants() {
        let source = "[other]\nfoo = 1\n";
        let grants = parse_extensions_toml(source).unwrap();
        assert!(grants.is_empty());
    }

    #[test]
    fn empty_source_is_empty_grants() {
        let grants = parse_extensions_toml("").unwrap();
        assert!(grants.is_empty());
    }

    #[test]
    fn malformed_toml_surfaces_typed_error() {
        let err = parse_extensions_toml("this is not toml = = =").unwrap_err();
        assert!(matches!(err, GrantsError::Toml(_)));
    }
}
