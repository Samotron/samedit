//! `cockpit-http` — Bruno-compatible HTTP request collection model (v0.11).
//!
//! Parses [`.bru`](https://docs.usebruno.com/bru-lang/overview.html) files
//! into typed [`Request`]s, serialises them back, and groups them into
//! [`Collection`]s. Pure data + parsing today; the HTTP engine, view-model,
//! and Lua scripting layers (M11.3+) land in follow-up milestones.
//!
//! Scope of this first cut:
//! - The block-structured `.bru` grammar — `meta`, HTTP verb (`get`,
//!   `post`, `put`, `patch`, `delete`, `head`, `options`), `headers`,
//!   `query`, `body:<kind>`, `auth:<kind>`, `docs`.
//! - Round-tripping through [`Request`] is *semantically* preserved —
//!   parsing then re-serialising yields a canonical form, not the exact
//!   bytes (block ordering normalises, leading whitespace inside braces
//!   collapses). The richer comment- and ordering-preserving editor lands
//!   alongside the M11.4 view-model.
//!
//! Stays headless and unit-tested per the AGENTS.md hard rules — no
//! network, no async runtime, no UI dependencies.

pub mod collection;
pub mod model;
pub mod parse;
pub mod serialise;

pub use collection::{
    COLLECTION_DIR, COLLECTION_MARKER, CollectionError, CollectionRoot, ENVIRONMENTS_DIR,
    InterpolateError, detect_collection_root, interpolate, load_collection,
};
pub use model::{
    Auth, BasicAuth, BearerAuth, Body, Collection, Environment, HttpMethod, Meta, Request,
};
pub use parse::{ParseError, parse_request};
pub use serialise::serialise_request;
