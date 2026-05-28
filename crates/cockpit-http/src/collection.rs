//! Collection + environment loader (v0.11 M11.2).
//!
//! A Bruno collection is a directory of `.bru` request files plus an
//! `environments/` subdirectory of environment files. Cockpit recognises a
//! project as a collection when one of the following is true:
//!
//! - A `bruno.json` file exists at the project root.
//! - A `cockpit-http/` directory exists at the project root.
//!
//! Either way the loader walks the project tree (skipping `environments/`
//! while collecting requests, and treating it specially when collecting
//! environments) and parses every `.bru` it finds. Headless: callers pass
//! the project root, the loader returns a typed [`Collection`].
//!
//! Variable interpolation (`{{varName}}`) lives in [`interpolate`] —
//! resolves against an active [`Environment`], detects cycles, surfaces
//! missing variables as typed errors. The HTTP engine (M11.3) consumes
//! the interpolated string; the parser does not.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::model::{Collection, Environment, Request};
use crate::parse::{ParseError, parse_request};

/// Filename that, when present at the project root, marks the project as a
/// Bruno collection. Mirrors the upstream Bruno desktop app's marker.
pub const COLLECTION_MARKER: &str = "bruno.json";
/// Directory that, when present at the project root, also marks the
/// project as a cockpit-managed Bruno collection. Lets users opt in
/// without committing the Bruno desktop marker.
pub const COLLECTION_DIR: &str = "cockpit-http";
/// Sub-directory inside the collection that holds environment files.
pub const ENVIRONMENTS_DIR: &str = "environments";

/// Result of probing a project directory for a Bruno collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionRoot {
    /// Collection root is the project root itself (`bruno.json` is present).
    ProjectRoot,
    /// Collection root is `<project>/cockpit-http/`.
    Subdirectory,
}

impl CollectionRoot {
    /// Resolve the absolute collection root for a given project root.
    pub fn resolve(self, project_root: &Path) -> PathBuf {
        match self {
            CollectionRoot::ProjectRoot => project_root.to_path_buf(),
            CollectionRoot::Subdirectory => project_root.join(COLLECTION_DIR),
        }
    }
}

/// Detect whether `project_root` looks like a Bruno collection.
pub fn detect_collection_root(project_root: &Path) -> Option<CollectionRoot> {
    if project_root.join(COLLECTION_MARKER).is_file() {
        return Some(CollectionRoot::ProjectRoot);
    }
    if project_root.join(COLLECTION_DIR).is_dir() {
        return Some(CollectionRoot::Subdirectory);
    }
    None
}

/// Load a [`Collection`] rooted at `project_root`. Returns `Ok(None)`
/// when the project is not a Bruno collection (no marker, no
/// `cockpit-http/`). Errors only on actual I/O / parse failure under a
/// detected collection.
pub fn load_collection(project_root: &Path) -> Result<Option<Collection>, CollectionError> {
    let Some(kind) = detect_collection_root(project_root) else {
        return Ok(None);
    };
    let root = kind.resolve(project_root);
    let requests = load_requests(&root)?;
    let environments = load_environments(&root)?;
    Ok(Some(Collection {
        root,
        requests,
        environments,
    }))
}

/// Recursively walk `root`, parsing every `.bru` file except those
/// inside the `environments/` subdirectory. Returned requests are
/// sorted by file path so collection ordering is stable.
fn load_requests(root: &Path) -> Result<Vec<Request>, CollectionError> {
    let mut paths = Vec::new();
    walk_bru_files(root, &mut paths, /* include_environments */ false)?;
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let source = fs::read_to_string(&path).map_err(|err| CollectionError::Read {
            path: path.clone(),
            source: err,
        })?;
        let request = parse_request(&source).map_err(|err| CollectionError::Parse {
            path: path.clone(),
            source: err,
        })?;
        out.push(request);
    }
    Ok(out)
}

fn load_environments(root: &Path) -> Result<Vec<Environment>, CollectionError> {
    let env_dir = root.join(ENVIRONMENTS_DIR);
    if !env_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(&env_dir)
        .map_err(|err| CollectionError::Read {
            path: env_dir.clone(),
            source: err,
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("bru"))
        .collect();
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let source = fs::read_to_string(&path).map_err(|err| CollectionError::Read {
            path: path.clone(),
            source: err,
        })?;
        let env = parse_environment(&path, &source)?;
        out.push(env);
    }
    Ok(out)
}

/// Parse a single environment file. Environment files use the same
/// block syntax as requests, but contain only a `vars { ... }` block.
/// Extra blocks are tolerated (silently skipped) for forward-compat.
fn parse_environment(path: &Path, source: &str) -> Result<Environment, CollectionError> {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("env")
        .to_string();
    let mut env = Environment {
        name,
        vars: BTreeMap::new(),
    };
    // Hand-roll a tiny block scanner here rather than reaching into the
    // request parser's internals — environments are simpler (one block
    // shape) and the request parser bails when it can't find an HTTP
    // verb.
    let mut iter = source.lines().enumerate().peekable();
    while let Some((index, line)) = iter.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(header) = trimmed.strip_suffix('{') else {
            return Err(CollectionError::Parse {
                path: path.to_path_buf(),
                source: ParseError::ExpectedBlockHeader {
                    line: index + 1,
                    snippet: trimmed.to_string(),
                },
            });
        };
        let header = header.trim();
        let mut body = String::new();
        let mut closed = false;
        for (_, inner) in iter.by_ref() {
            if inner == "}" {
                closed = true;
                break;
            }
            body.push_str(inner);
            body.push('\n');
        }
        if !closed {
            return Err(CollectionError::Parse {
                path: path.to_path_buf(),
                source: ParseError::UnclosedBlock {
                    line: index + 1,
                    header: header.to_string(),
                },
            });
        }
        if header == "vars" {
            for (offset, raw) in body.lines().enumerate() {
                let line = raw.trim();
                if line.is_empty() {
                    continue;
                }
                let stripped = line.strip_prefix('~').map(str::trim_start).unwrap_or(line);
                let disabled = line.starts_with('~');
                let Some((key, value)) = stripped.split_once(':') else {
                    return Err(CollectionError::Parse {
                        path: path.to_path_buf(),
                        source: ParseError::MalformedKeyValue {
                            line: index + offset + 2,
                            snippet: line.to_string(),
                        },
                    });
                };
                if disabled {
                    continue;
                }
                env.vars
                    .insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    Ok(env)
}

fn walk_bru_files(
    dir: &Path,
    out: &mut Vec<PathBuf>,
    include_environments: bool,
) -> Result<(), CollectionError> {
    let entries = fs::read_dir(dir).map_err(|err| CollectionError::Read {
        path: dir.to_path_buf(),
        source: err,
    })?;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| CollectionError::Read {
            path: path.clone(),
            source: err,
        })?;
        if file_type.is_dir() {
            if !include_environments
                && path.file_name().and_then(|name| name.to_str()) == Some(ENVIRONMENTS_DIR)
            {
                continue;
            }
            walk_bru_files(&path, out, include_environments)?;
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("bru")
        {
            out.push(path);
        }
    }
    Ok(())
}

/// Resolve `{{varName}}` placeholders in `template` against `env`.
///
/// Variable values may themselves reference other variables; the resolver
/// expands them transitively and detects cycles. Unknown references
/// surface as [`InterpolateError::MissingVariable`]. Lookups outside `{{
/// ... }}` braces are returned untouched.
pub fn interpolate(template: &str, env: &Environment) -> Result<String, InterpolateError> {
    let mut visiting = HashSet::new();
    expand(template, env, &mut visiting)
}

fn expand(
    template: &str,
    env: &Environment,
    visiting: &mut HashSet<String>,
) -> Result<String, InterpolateError> {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            // Find the matching `}}`.
            let rest = &template[i + 2..];
            let Some(end_rel) = rest.find("}}") else {
                // No closer — treat the rest as literal.
                out.push_str(&template[i..]);
                break;
            };
            let name = rest[..end_rel].trim().to_string();
            if name.is_empty() {
                return Err(InterpolateError::EmptyPlaceholder);
            }
            if !visiting.insert(name.clone()) {
                return Err(InterpolateError::Cycle { name });
            }
            let Some(value) = env.vars.get(&name) else {
                return Err(InterpolateError::MissingVariable { name });
            };
            let expanded = expand(value, env, visiting)?;
            visiting.remove(&name);
            out.push_str(&expanded);
            i += 2 + end_rel + 2;
        } else {
            // Push the next char as a whole UTF-8 unit.
            let ch = template[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Ok(out)
}

/// Failure modes for [`load_collection`].
#[derive(Debug, Error)]
pub enum CollectionError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse `{path}`: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: ParseError,
    },
}

/// Failure modes for [`interpolate`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum InterpolateError {
    #[error("variable `{name}` is not defined in the active environment")]
    MissingVariable { name: String },
    #[error("variable `{name}` participates in a reference cycle")]
    Cycle { name: String },
    #[error("placeholder `{{{{}}}}` has an empty variable name")]
    EmptyPlaceholder,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn env(pairs: &[(&str, &str)]) -> Environment {
        let mut vars = BTreeMap::new();
        for (k, v) in pairs {
            vars.insert((*k).to_string(), (*v).to_string());
        }
        Environment {
            name: "test".to_string(),
            vars,
        }
    }

    #[test]
    fn interpolate_substitutes_known_variables() {
        let env = env(&[("base", "https://api.example.com"), ("user", "alice")]);
        let out = interpolate("{{base}}/users/{{user}}", &env).unwrap();
        assert_eq!(out, "https://api.example.com/users/alice");
    }

    #[test]
    fn interpolate_supports_transitive_references() {
        let env = env(&[("base", "https://{{host}}"), ("host", "api.example.com")]);
        let out = interpolate("{{base}}/v1", &env).unwrap();
        assert_eq!(out, "https://api.example.com/v1");
    }

    #[test]
    fn interpolate_detects_cycles() {
        let env = env(&[("a", "{{b}}"), ("b", "{{a}}")]);
        let err = interpolate("{{a}}", &env).unwrap_err();
        assert!(matches!(err, InterpolateError::Cycle { .. }), "{err:?}");
    }

    #[test]
    fn interpolate_surfaces_missing_variable() {
        let env = env(&[]);
        let err = interpolate("{{missing}}", &env).unwrap_err();
        assert_eq!(
            err,
            InterpolateError::MissingVariable {
                name: "missing".to_string()
            }
        );
    }

    #[test]
    fn interpolate_passes_through_non_placeholder_braces() {
        let env = env(&[]);
        let out = interpolate("a { b } c", &env).unwrap();
        assert_eq!(out, "a { b } c");
    }

    #[test]
    fn interpolate_rejects_empty_placeholder() {
        let env = env(&[]);
        let err = interpolate("{{   }}", &env).unwrap_err();
        assert_eq!(err, InterpolateError::EmptyPlaceholder);
    }

    #[test]
    fn detect_collection_root_finds_bruno_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(COLLECTION_MARKER), "{}").unwrap();
        assert_eq!(
            detect_collection_root(dir.path()),
            Some(CollectionRoot::ProjectRoot)
        );
    }

    #[test]
    fn detect_collection_root_finds_subdirectory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(COLLECTION_DIR)).unwrap();
        assert_eq!(
            detect_collection_root(dir.path()),
            Some(CollectionRoot::Subdirectory)
        );
    }

    #[test]
    fn detect_collection_root_returns_none_for_a_plain_project() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(detect_collection_root(dir.path()), None);
    }

    #[test]
    fn load_collection_returns_none_for_non_bruno_projects() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_collection(dir.path()).unwrap().is_none());
    }

    #[test]
    fn load_collection_parses_requests_and_environments() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(COLLECTION_MARKER), "{}").unwrap();
        std::fs::write(
            dir.path().join("get.bru"),
            "meta {\n  name: Get One\n}\n\nget {\n  url: {{base}}/one\n  body: none\n  auth: none\n}\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join(ENVIRONMENTS_DIR)).unwrap();
        std::fs::write(
            dir.path().join(ENVIRONMENTS_DIR).join("local.bru"),
            "vars {\n  base: http://localhost\n  ~disabled: skip\n}\n",
        )
        .unwrap();
        let collection = load_collection(dir.path()).unwrap().expect("collection");
        assert_eq!(collection.requests.len(), 1);
        assert_eq!(collection.requests[0].meta.name.as_deref(), Some("Get One"));
        assert_eq!(collection.environments.len(), 1);
        let env = &collection.environments[0];
        assert_eq!(env.name, "local");
        assert_eq!(
            env.vars.get("base").map(String::as_str),
            Some("http://localhost")
        );
        assert!(
            !env.vars.contains_key("disabled"),
            "disabled rows must not contribute to the env"
        );
    }

    #[test]
    fn load_collection_skips_bru_files_inside_environments_when_collecting_requests() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(COLLECTION_DIR)).unwrap();
        let root = dir.path().join(COLLECTION_DIR);
        std::fs::write(
            root.join("real.bru"),
            "get {\n  url: http://a\n  body: none\n  auth: none\n}\n",
        )
        .unwrap();
        std::fs::create_dir(root.join(ENVIRONMENTS_DIR)).unwrap();
        std::fs::write(
            root.join(ENVIRONMENTS_DIR).join("dev.bru"),
            "vars {\n  base: http://dev\n}\n",
        )
        .unwrap();
        let collection = load_collection(dir.path()).unwrap().expect("collection");
        assert_eq!(collection.requests.len(), 1, "request count");
        assert_eq!(collection.environments.len(), 1, "env count");
    }
}
