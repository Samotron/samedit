//! Typed data model for a Bruno collection.
//!
//! Mirrors the on-disk `.bru` shape closely so the round-trip stays simple:
//! one [`Request`] per file, optional [`Meta`], an [`HttpMethod`] + URL, and
//! the auxiliary lists Bruno splits into separate blocks (headers, query,
//! body, auth, docs).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One `.bru` file parsed into typed form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub meta: Meta,
    pub method: HttpMethod,
    /// Verb-block fields: `url`, `body`, `auth` — Bruno lists these inside
    /// the verb block; we lift `url` out for ergonomics and keep the rest
    /// as raw key→value so unknown keys round-trip untouched.
    pub url: String,
    pub body_kind: String,
    pub auth_kind: String,
    pub headers: Vec<KeyValue>,
    pub query: Vec<KeyValue>,
    pub body: Body,
    pub auth: Auth,
    pub docs: Option<String>,
    /// Cockpit-Lua pre-request script (`script:lua-pre-request` block,
    /// v0.11 M11.6). Runs before [`prepare_request`] each send; mutates
    /// environment variables through the sandboxed `cockpit.http.*`
    /// surface. `None` when no such block is present.
    #[serde(default)]
    pub pre_script: Option<String>,
    /// Cockpit-Lua post-response script (`script:lua-post-response`
    /// block, v0.11 M11.6). Runs after the engine returns; sees a
    /// read-only snapshot of the response.
    #[serde(default)]
    pub post_script: Option<String>,
    /// `true` when the source file declared `script:js-pre-request` or
    /// `script:js-post-response`. Cockpit does not run JS — the M11.6
    /// view-model surfaces a toast on first run so the user knows their
    /// Bruno-format script is being skipped.
    #[serde(default)]
    pub has_js_scripts: bool,
}

impl Request {
    /// A minimal valid request: empty meta, GET, empty URL, no extras.
    pub fn empty() -> Self {
        Self {
            meta: Meta::default(),
            method: HttpMethod::Get,
            url: String::new(),
            body_kind: "none".to_string(),
            auth_kind: "none".to_string(),
            headers: Vec::new(),
            query: Vec::new(),
            body: Body::None,
            auth: Auth::None,
            docs: None,
            pre_script: None,
            post_script: None,
            has_js_scripts: false,
        }
    }
}

/// `meta { ... }` block. All fields optional so Bruno's
/// minimal-collection-without-meta still round-trips.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Meta {
    pub name: Option<String>,
    pub seq: Option<u32>,
    /// Bruno's `type` is `http` or `graphql`; we keep it as a string so a
    /// future GraphQL pass doesn't need a model change.
    pub kind: Option<String>,
}

/// HTTP verb. Bruno spells out each as its own block name (`get`, `post`,
/// `put`, `patch`, `delete`, `head`, `options`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl HttpMethod {
    /// Block name as Bruno spells it (lowercase verb).
    pub fn block_name(self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Patch => "patch",
            HttpMethod::Delete => "delete",
            HttpMethod::Head => "head",
            HttpMethod::Options => "options",
        }
    }

    /// Parse a block name; returns `None` if the name isn't a known verb.
    pub fn from_block_name(name: &str) -> Option<Self> {
        match name {
            "get" => Some(HttpMethod::Get),
            "post" => Some(HttpMethod::Post),
            "put" => Some(HttpMethod::Put),
            "patch" => Some(HttpMethod::Patch),
            "delete" => Some(HttpMethod::Delete),
            "head" => Some(HttpMethod::Head),
            "options" => Some(HttpMethod::Options),
            _ => None,
        }
    }
}

/// One key=value pair in a `headers`, `query`, or `vars:*` block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    /// Bruno marks disabled rows with a leading `~`. Preserved so the user's
    /// "save commented-out" header stays commented-out across edits.
    #[serde(default)]
    pub disabled: bool,
}

impl KeyValue {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            disabled: false,
        }
    }
}

/// Request body. Bruno calls the variant out in the block name
/// (`body:json`, `body:text`, …); the body block then holds the bytes
/// (free-text until the matching brace at column 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Body {
    None,
    Text(String),
    Json(String),
    Xml(String),
    Form(Vec<KeyValue>),
}

impl Body {
    /// The Bruno suffix on the `body:<kind>` block name (`"none"` collapses
    /// to no body block being emitted).
    pub fn kind(&self) -> &'static str {
        match self {
            Body::None => "none",
            Body::Text(_) => "text",
            Body::Json(_) => "json",
            Body::Xml(_) => "xml",
            Body::Form(_) => "form-urlencoded",
        }
    }
}

/// Authentication scheme. Bruno keeps each in its own `auth:<kind>` block;
/// the verb block then declares which one is active in its `auth:` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    None,
    Basic(BasicAuth),
    Bearer(BearerAuth),
}

impl Auth {
    pub fn kind(&self) -> &'static str {
        match self {
            Auth::None => "none",
            Auth::Basic(_) => "basic",
            Auth::Bearer(_) => "bearer",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BearerAuth {
    pub token: String,
}

/// One environment file (Bruno's `environments/<name>.bru`). The vars
/// map preserves declared order via [`BTreeMap`] so equality between two
/// environments depends only on content, not insertion order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub name: String,
    pub vars: BTreeMap<String, String>,
}

/// A Bruno collection — root directory + parsed [`Request`]s + environments.
/// The full collection loader (filesystem walk, environment activation) is
/// M11.2; for now this is just the typed shell.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    pub root: PathBuf,
    pub requests: Vec<Request>,
    pub environments: Vec<Environment>,
}
