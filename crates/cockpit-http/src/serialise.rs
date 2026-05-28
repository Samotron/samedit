//! Serialise a [`Request`] back into Bruno's `.bru` text format.
//!
//! Output is canonical: blocks come out in a fixed order (`meta`, verb,
//! `headers`, `query`, `body:*`, `auth:*`, `docs`), key/value pairs are
//! emitted at two-space indentation, and disabled rows keep their leading
//! `~`. The result is what cockpit writes when the user edits a request
//! through the (future M11.4) view-model. Round-tripping is *semantic*,
//! not byte-identical — the parser is lenient about whitespace and block
//! order, but the serialiser is opinionated about both.
//!
//! A future revision will switch to an edit-AST that preserves the user's
//! original formatting (similar to `toml_edit` for `cockpit-config`); the
//! canonical form here is enough for the M11.1 round-trip-correctness
//! tests.

use std::fmt::Write;

use crate::model::{Auth, Body, KeyValue, Request};

/// Render `request` back into `.bru` text.
pub fn serialise_request(request: &Request) -> String {
    let mut out = String::new();
    serialise_meta(&mut out, request);
    serialise_verb(&mut out, request);
    serialise_kv_block(&mut out, "headers", &request.headers);
    serialise_kv_block(&mut out, "query", &request.query);
    serialise_body(&mut out, &request.body);
    serialise_auth(&mut out, &request.auth);
    serialise_docs(&mut out, request);
    out
}

fn serialise_meta(out: &mut String, request: &Request) {
    let meta = &request.meta;
    if meta.name.is_none() && meta.seq.is_none() && meta.kind.is_none() {
        return;
    }
    out.push_str("meta {\n");
    if let Some(name) = &meta.name {
        let _ = writeln!(out, "  name: {name}");
    }
    if let Some(kind) = &meta.kind {
        let _ = writeln!(out, "  type: {kind}");
    }
    if let Some(seq) = meta.seq {
        let _ = writeln!(out, "  seq: {seq}");
    }
    out.push_str("}\n\n");
}

fn serialise_verb(out: &mut String, request: &Request) {
    let _ = writeln!(out, "{} {{", request.method.block_name());
    let _ = writeln!(out, "  url: {}", request.url);
    let _ = writeln!(out, "  body: {}", body_kind_for_verb(request));
    let _ = writeln!(out, "  auth: {}", auth_kind_for_verb(request));
    out.push_str("}\n\n");
}

/// What the verb block's `body:` field should advertise. The body block
/// (`body:json`, etc.) is the source of truth when a payload is present;
/// the explicit [`Request::body_kind`] only matters when the body itself
/// is empty (e.g. a GET whose verb block still spells `body: none`).
fn body_kind_for_verb(request: &Request) -> &str {
    match request.body {
        Body::None => request.body_kind.as_str(),
        _ => request.body.kind(),
    }
}

fn auth_kind_for_verb(request: &Request) -> &str {
    match request.auth {
        Auth::None => request.auth_kind.as_str(),
        _ => request.auth.kind(),
    }
}

fn serialise_kv_block(out: &mut String, name: &str, entries: &[KeyValue]) {
    if entries.is_empty() {
        return;
    }
    let _ = writeln!(out, "{name} {{");
    for kv in entries {
        let prefix = if kv.disabled { "~" } else { "" };
        let _ = writeln!(out, "  {prefix}{}: {}", kv.key, kv.value);
    }
    out.push_str("}\n\n");
}

fn serialise_body(out: &mut String, body: &Body) {
    match body {
        Body::None => {}
        Body::Text(text) | Body::Json(text) | Body::Xml(text) => {
            let _ = writeln!(out, "body:{} {{", body.kind());
            write_indented_body(out, text);
            out.push_str("}\n\n");
        }
        Body::Form(entries) => serialise_kv_block(out, "body:form-urlencoded", entries),
    }
}

fn serialise_docs(out: &mut String, request: &Request) {
    let Some(docs) = &request.docs else { return };
    out.push_str("docs {\n");
    write_indented_body(out, docs);
    out.push_str("}\n");
}

/// Write `text` with every line indented by two spaces. Bruno's block
/// closer is a `}` at *column 0*, so body lines must never start at
/// column 0 — a JSON `}` on the user's payload would otherwise terminate
/// the body block early. The parser's `dedent_body` step removes this
/// indent on re-parse so the round-trip stays byte-stable.
fn write_indented_body(out: &mut String, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "  {line}");
        }
    }
}

fn serialise_auth(out: &mut String, auth: &Auth) {
    match auth {
        Auth::None => {}
        Auth::Basic(basic) => {
            out.push_str("auth:basic {\n");
            let _ = writeln!(out, "  username: {}", basic.username);
            let _ = writeln!(out, "  password: {}", basic.password);
            out.push_str("}\n\n");
        }
        Auth::Bearer(bearer) => {
            out.push_str("auth:bearer {\n");
            let _ = writeln!(out, "  token: {}", bearer.token);
            out.push_str("}\n\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{HttpMethod, KeyValue, Meta};
    use crate::parse::parse_request;

    /// Build a request, serialise it, parse the serialised form, and check
    /// the two model values match. Round-trip via canonical form, not via
    /// byte-identical text — the serialiser is opinionated about layout.
    fn assert_canonical_round_trip(request: &Request) {
        let rendered = serialise_request(request);
        let reparsed = parse_request(&rendered).unwrap_or_else(|err| {
            panic!("serialised output failed to reparse: {err}\n--- rendered ---\n{rendered}")
        });
        assert_eq!(reparsed, *request, "canonical round-trip diverged");
    }

    #[test]
    fn empty_get_round_trips() {
        let mut request = Request::empty();
        request.url = "https://api.example.com/health".to_string();
        assert_canonical_round_trip(&request);
    }

    #[test]
    fn rich_request_round_trips() {
        let request = Request {
            meta: Meta {
                name: Some("List Users".to_string()),
                seq: Some(1),
                kind: Some("http".to_string()),
            },
            method: HttpMethod::Get,
            url: "https://api.example.com/users".to_string(),
            body_kind: "none".to_string(),
            auth_kind: "bearer".to_string(),
            headers: vec![
                KeyValue::new("Accept", "application/json"),
                KeyValue {
                    key: "X-Disabled".to_string(),
                    value: "ignored".to_string(),
                    disabled: true,
                },
            ],
            query: vec![KeyValue::new("limit", "10")],
            body: Body::None,
            auth: Auth::Bearer(crate::model::BearerAuth {
                token: "{{authToken}}".to_string(),
            }),
            docs: Some("Lists every user.".to_string()),
        };
        assert_canonical_round_trip(&request);
    }

    #[test]
    fn json_body_round_trips() {
        let request = Request {
            meta: Meta {
                name: Some("Create User".to_string()),
                ..Meta::default()
            },
            method: HttpMethod::Post,
            url: "https://api.example.com/users".to_string(),
            body_kind: "json".to_string(),
            auth_kind: "basic".to_string(),
            headers: Vec::new(),
            query: Vec::new(),
            body: Body::Json("{\n  \"name\": \"alice\"\n}".to_string()),
            auth: Auth::Basic(crate::model::BasicAuth {
                username: "alice".to_string(),
                password: "hunter2".to_string(),
            }),
            docs: None,
        };
        assert_canonical_round_trip(&request);
    }

    #[test]
    fn serialised_get_is_human_readable() {
        let request = Request {
            url: "https://api.example.com/users".to_string(),
            body_kind: "none".to_string(),
            auth_kind: "none".to_string(),
            ..Request::empty()
        };
        let rendered = serialise_request(&request);
        let expected =
            "get {\n  url: https://api.example.com/users\n  body: none\n  auth: none\n}\n\n";
        assert_eq!(rendered, expected);
    }
}
