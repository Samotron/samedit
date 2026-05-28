//! `.bru` parser.
//!
//! Bruno's file format is block-structured: each block begins with a name
//! (optionally suffixed by `:<variant>`), then a brace `{`, then either
//! key/value lines or — for body blocks — free-text bytes, then a closing
//! brace `}` at column 0. The parser scans line-by-line.
//!
//! What's recognised: `meta`, the seven HTTP verbs as blocks (`get`,
//! `post`, …), `headers`, `query`, `body:<kind>`, `auth:<kind>`, and
//! `docs`. Unknown blocks are ignored (with the assumption that the M11.4
//! editor will surface them as a "preserved but not edited" hint).
//!
//! Errors return a typed [`ParseError`] tagged with the source line; the
//! parser never panics on malformed input.

use std::str::FromStr;

use thiserror::Error;

use crate::model::{Auth, BasicAuth, BearerAuth, Body, HttpMethod, KeyValue, Meta, Request};

/// Parse a single `.bru` file into a [`Request`].
pub fn parse_request(input: &str) -> Result<Request, ParseError> {
    let blocks = lex_blocks(input)?;
    let mut request = Request::empty();
    let mut method_seen = false;

    for block in &blocks {
        let header = &block.header;
        let body = &block.body;
        match header.as_str() {
            "meta" => request.meta = parse_meta(body, block.start_line)?,
            "headers" => request.headers = parse_key_values(body, block.start_line)?,
            "query" => request.query = parse_key_values(body, block.start_line)?,
            "docs" => request.docs = Some(dedent_body(body)),
            other if HttpMethod::from_block_name(other).is_some() => {
                if method_seen {
                    return Err(ParseError::DuplicateVerb {
                        line: block.start_line,
                    });
                }
                method_seen = true;
                request.method = HttpMethod::from_block_name(other).unwrap();
                let (url, body_kind, auth_kind) = parse_verb_block(body, block.start_line)?;
                request.url = url;
                request.body_kind = body_kind;
                request.auth_kind = auth_kind;
            }
            other if let Some(kind) = other.strip_prefix("body:") => {
                request.body = parse_body(kind, body, block.start_line)?;
            }
            other if let Some(kind) = other.strip_prefix("auth:") => {
                request.auth = parse_auth(kind, body, block.start_line)?;
            }
            _ => {
                // Silently ignore unknown blocks for forward-compat. The
                // M11.4 view-model will surface them as preserved-but-
                // not-edited; for now they round-trip via the serialiser's
                // own emission, not through the parser.
            }
        }
    }

    if !method_seen {
        return Err(ParseError::MissingVerb);
    }
    Ok(request)
}

/// One lexed `<header> { <body> }` block, with the source line of the
/// opening brace for diagnostics.
#[derive(Debug)]
struct Block {
    header: String,
    body: String,
    start_line: usize,
}

/// Split `input` into [`Block`]s. Block bodies are everything between the
/// `{` on the header line and a `}` that appears at column 0 (Bruno's
/// indentation rule — body blocks can contain `}` characters as long as
/// they're indented).
fn lex_blocks(input: &str) -> Result<Vec<Block>, ParseError> {
    let mut blocks = Vec::new();
    let mut iter = input.lines().enumerate().peekable();
    while let Some((index, line)) = iter.next() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(header) = trimmed.strip_suffix('{') else {
            return Err(ParseError::ExpectedBlockHeader {
                line: line_no,
                snippet: trimmed.to_string(),
            });
        };
        let header = header.trim().to_string();
        if header.is_empty() {
            return Err(ParseError::ExpectedBlockHeader {
                line: line_no,
                snippet: trimmed.to_string(),
            });
        }
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
            return Err(ParseError::UnclosedBlock {
                line: line_no,
                header,
            });
        }
        blocks.push(Block {
            header,
            body,
            start_line: line_no,
        });
    }
    Ok(blocks)
}

/// Parse `key: value` lines inside a block. Lines beginning with `~` are
/// disabled (Bruno's commented-out marker). Blank lines are ignored.
fn parse_key_values(body: &str, start_line: usize) -> Result<Vec<KeyValue>, ParseError> {
    let mut out = Vec::new();
    for (offset, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (disabled, rest) = if let Some(stripped) = line.strip_prefix('~') {
            (true, stripped.trim_start())
        } else {
            (false, line)
        };
        let Some((key, value)) = rest.split_once(':') else {
            return Err(ParseError::MalformedKeyValue {
                line: start_line + offset + 1,
                snippet: line.to_string(),
            });
        };
        out.push(KeyValue {
            key: key.trim().to_string(),
            value: value.trim().to_string(),
            disabled,
        });
    }
    Ok(out)
}

fn parse_meta(body: &str, start_line: usize) -> Result<Meta, ParseError> {
    let kvs = parse_key_values(body, start_line)?;
    let mut meta = Meta::default();
    for kv in kvs {
        if kv.disabled {
            continue;
        }
        match kv.key.as_str() {
            "name" => meta.name = Some(kv.value),
            "seq" => {
                let parsed =
                    u32::from_str(&kv.value).map_err(|_| ParseError::MalformedKeyValue {
                        line: start_line,
                        snippet: format!("seq: {} (expected an unsigned integer)", kv.value),
                    })?;
                meta.seq = Some(parsed);
            }
            "type" => meta.kind = Some(kv.value),
            _ => {} // Unknown meta keys round-trip via the model's `extra` hook later.
        }
    }
    Ok(meta)
}

fn parse_verb_block(body: &str, start_line: usize) -> Result<(String, String, String), ParseError> {
    let kvs = parse_key_values(body, start_line)?;
    let mut url = String::new();
    let mut body_kind = "none".to_string();
    let mut auth_kind = "none".to_string();
    for kv in kvs {
        match kv.key.as_str() {
            "url" => url = kv.value,
            "body" => body_kind = kv.value,
            "auth" => auth_kind = kv.value,
            _ => {}
        }
    }
    Ok((url, body_kind, auth_kind))
}

fn parse_body(kind: &str, body: &str, start_line: usize) -> Result<Body, ParseError> {
    match kind {
        "none" => Ok(Body::None),
        "text" => Ok(Body::Text(dedent_body(body))),
        "json" => Ok(Body::Json(dedent_body(body))),
        "xml" => Ok(Body::Xml(dedent_body(body))),
        "form-urlencoded" => Ok(Body::Form(parse_key_values(body, start_line)?)),
        other => Err(ParseError::UnknownBodyKind {
            line: start_line,
            kind: other.to_string(),
        }),
    }
}

/// Drop fully-blank leading/trailing lines and remove the minimum common
/// leading-space indent from the remaining lines. Matches the
/// canonicalisation `serialise::write_indented_body` applies on the way
/// out so that parse → serialise → parse stays byte-stable.
fn dedent_body(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut start = 0;
    let mut end = lines.len();
    while start < end && lines[start].trim().is_empty() {
        start += 1;
    }
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    if start == end {
        return String::new();
    }
    let trimmed = &lines[start..end];
    let indent = trimmed
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, line) in trimmed.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.len() >= indent {
            out.push_str(&line[indent..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

fn parse_auth(kind: &str, body: &str, start_line: usize) -> Result<Auth, ParseError> {
    match kind {
        "none" => Ok(Auth::None),
        "basic" => {
            let mut basic = BasicAuth::default();
            for kv in parse_key_values(body, start_line)? {
                match kv.key.as_str() {
                    "username" => basic.username = kv.value,
                    "password" => basic.password = kv.value,
                    _ => {}
                }
            }
            Ok(Auth::Basic(basic))
        }
        "bearer" => {
            let mut bearer = BearerAuth::default();
            for kv in parse_key_values(body, start_line)? {
                if kv.key == "token" {
                    bearer.token = kv.value;
                }
            }
            Ok(Auth::Bearer(bearer))
        }
        other => Err(ParseError::UnknownAuthKind {
            line: start_line,
            kind: other.to_string(),
        }),
    }
}

/// A `.bru` parse error.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("expected `<block> {{` on line {line}, found `{snippet}`")]
    ExpectedBlockHeader { line: usize, snippet: String },
    #[error("block `{header}` opened on line {line} is never closed")]
    UnclosedBlock { line: usize, header: String },
    #[error("malformed key/value pair on line {line}: `{snippet}` (expected `key: value`)")]
    MalformedKeyValue { line: usize, snippet: String },
    #[error("request defines more than one HTTP verb (second on line {line})")]
    DuplicateVerb { line: usize },
    #[error("request defines no HTTP verb block (expected `get`, `post`, …)")]
    MissingVerb,
    #[error("unknown body kind on line {line}: `{kind}`")]
    UnknownBodyKind { line: usize, kind: String },
    #[error("unknown auth kind on line {line}: `{kind}`")]
    UnknownAuthKind { line: usize, kind: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    const GET_REQUEST: &str = r#"meta {
  name: List Users
  seq: 1
  type: http
}

get {
  url: https://api.example.com/users
  body: none
  auth: bearer
}

headers {
  Accept: application/json
  ~X-Disabled: ignored
}

query {
  limit: 10
  page: 2
}

auth:bearer {
  token: {{authToken}}
}

docs {
  Lists every user in the system.
}
"#;

    #[test]
    fn parses_a_typical_get_request() {
        let request = parse_request(GET_REQUEST).expect("parses cleanly");
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.url, "https://api.example.com/users");
        assert_eq!(request.meta.name.as_deref(), Some("List Users"));
        assert_eq!(request.meta.seq, Some(1));
        assert_eq!(request.meta.kind.as_deref(), Some("http"));
        assert_eq!(request.body_kind, "none");
        assert_eq!(request.auth_kind, "bearer");
        assert_eq!(
            request.headers,
            vec![
                KeyValue::new("Accept", "application/json"),
                KeyValue {
                    key: "X-Disabled".to_string(),
                    value: "ignored".to_string(),
                    disabled: true,
                },
            ],
        );
        assert_eq!(
            request.query,
            vec![KeyValue::new("limit", "10"), KeyValue::new("page", "2"),],
        );
        match request.auth {
            Auth::Bearer(bearer) => assert_eq!(bearer.token, "{{authToken}}"),
            other => panic!("expected bearer auth, got {other:?}"),
        }
        assert!(matches!(request.body, Body::None));
        assert_eq!(
            request.docs.as_deref(),
            Some("Lists every user in the system."),
        );
    }

    #[test]
    fn parses_a_post_with_json_body_and_basic_auth() {
        let input = r#"meta {
  name: Create User
}

post {
  url: https://api.example.com/users
  body: json
  auth: basic
}

body:json {
  {
    "name": "alice",
    "email": "alice@example.com"
  }
}

auth:basic {
  username: alice
  password: hunter2
}
"#;
        let request = parse_request(input).expect("parses cleanly");
        assert_eq!(request.method, HttpMethod::Post);
        match request.body {
            Body::Json(json) => {
                assert!(json.contains("\"name\": \"alice\""), "{json}");
                // Outer blank lines are stripped; inner JSON whitespace stays.
                assert!(!json.starts_with('\n'));
                assert!(!json.ends_with('\n'));
            }
            other => panic!("expected JSON body, got {other:?}"),
        }
        match request.auth {
            Auth::Basic(basic) => {
                assert_eq!(basic.username, "alice");
                assert_eq!(basic.password, "hunter2");
            }
            other => panic!("expected basic auth, got {other:?}"),
        }
    }

    #[test]
    fn missing_verb_block_is_a_typed_error() {
        let input = "meta {\n  name: oops\n}\n";
        let err = parse_request(input).unwrap_err();
        assert_eq!(err, ParseError::MissingVerb);
    }

    #[test]
    fn duplicate_verb_blocks_are_rejected() {
        let input = "get {\n  url: http://a\n}\npost {\n  url: http://b\n}\n";
        let err = parse_request(input).unwrap_err();
        assert!(matches!(err, ParseError::DuplicateVerb { .. }));
    }

    #[test]
    fn unclosed_block_reports_the_opening_line() {
        let input = "get {\n  url: http://a\n";
        let err = parse_request(input).unwrap_err();
        match err {
            ParseError::UnclosedBlock { line, header } => {
                assert_eq!(line, 1);
                assert_eq!(header, "get");
            }
            other => panic!("expected UnclosedBlock, got {other:?}"),
        }
    }

    #[test]
    fn malformed_key_value_reports_line_inside_block() {
        let input = "get {\n  url: http://a\n}\n\nheaders {\n  Accept application/json\n}\n";
        let err = parse_request(input).unwrap_err();
        match err {
            ParseError::MalformedKeyValue { line, .. } => assert_eq!(line, 6),
            other => panic!("expected MalformedKeyValue, got {other:?}"),
        }
    }

    #[test]
    fn comments_and_blank_lines_at_top_level_are_skipped() {
        let input = "# Comment line at top.\n\nget {\n  url: http://a\n}\n";
        let request = parse_request(input).expect("parses cleanly");
        assert_eq!(request.url, "http://a");
    }

    #[test]
    fn unknown_block_at_top_level_is_ignored() {
        let input = "get {\n  url: http://a\n}\n\nfuture-block {\n  whatever: ok\n}\n";
        let request = parse_request(input).expect("parses cleanly");
        assert_eq!(request.url, "http://a");
    }

    #[test]
    fn every_verb_round_trips_through_from_block_name() {
        for verb in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
            HttpMethod::Head,
            HttpMethod::Options,
        ] {
            assert_eq!(HttpMethod::from_block_name(verb.block_name()), Some(verb));
        }
    }
}
