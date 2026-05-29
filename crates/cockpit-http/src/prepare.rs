//! Turn a parsed [`Request`] + active [`Environment`] into a
//! [`PreparedRequest`] the engine can ship (v0.11 M11.3).
//!
//! Responsibilities:
//!
//! - Interpolate `{{vars}}` in URL, header values, query values, body bytes,
//!   and auth fields.
//! - Skip disabled rows (Bruno marks them with a leading `~`).
//! - Append query parameters as a URL-encoded query string.
//! - Synthesise an `Authorization` header from the active `auth:*` block.
//! - Set the matching `Content-Type` for JSON / XML / form bodies (unless
//!   the user already declared one in `headers`).
//!
//! Pre/post-request Lua scripts (M11.6) wrap this step from the outside —
//! a script reads the prepared request, possibly mutates the environment,
//! then prepares again. Keeping `prepare_request` pure makes that loop
//! deterministic.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use thiserror::Error;

use crate::collection::{InterpolateError, interpolate};
use crate::engine::{PreparedBody, PreparedRequest};
use crate::model::{Auth, Body, Environment, KeyValue, Request};

/// Failure modes for [`prepare_request`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PrepareError {
    /// A `{{var}}` placeholder couldn't be resolved (or formed a cycle).
    #[error("variable interpolation failed in {field}: {source}")]
    Interpolation {
        /// Which logical field failed — `"url"`, `"header `<name>`"`,
        /// `"query"`, `"body"`, etc. Lets the UI focus the offending line.
        field: String,
        #[source]
        source: InterpolateError,
    },
    /// The request's URL was empty (Bruno permits this on disk, but the
    /// engine can't dispatch a no-URL request).
    #[error("request has no URL")]
    EmptyUrl,
}

/// Build a [`PreparedRequest`] from `req` against `env`.
///
/// Disabled rows (`~name: value` in headers / query) are stripped here, not
/// at the engine boundary, so the prepared form already matches what the
/// network will see.
pub fn prepare_request(req: &Request, env: &Environment) -> Result<PreparedRequest, PrepareError> {
    let url = interpolate(&req.url, env).map_err(|source| PrepareError::Interpolation {
        field: "url".into(),
        source,
    })?;
    if url.trim().is_empty() {
        return Err(PrepareError::EmptyUrl);
    }

    let mut headers: Vec<(String, String)> = Vec::with_capacity(req.headers.len());
    for kv in &req.headers {
        if kv.disabled {
            continue;
        }
        let value = interpolate_field(env, &kv.value, || format!("header `{}`", kv.key))?;
        headers.push((kv.key.clone(), value));
    }

    let url_with_query = append_query(&url, &req.query, env)?;

    let body = prepare_body(&req.body, env)?;
    apply_default_content_type(&mut headers, &body);
    apply_auth_header(&mut headers, &req.auth, env)?;

    Ok(PreparedRequest {
        method: req.method,
        url: url_with_query,
        headers,
        body,
        timeout: None,
    })
}

fn interpolate_field<F>(env: &Environment, value: &str, field: F) -> Result<String, PrepareError>
where
    F: FnOnce() -> String,
{
    interpolate(value, env).map_err(|source| PrepareError::Interpolation {
        field: field(),
        source,
    })
}

fn append_query(url: &str, query: &[KeyValue], env: &Environment) -> Result<String, PrepareError> {
    let live: Vec<&KeyValue> = query.iter().filter(|kv| !kv.disabled).collect();
    if live.is_empty() {
        return Ok(url.to_string());
    }
    let mut pairs = Vec::with_capacity(live.len());
    for kv in live {
        let value = interpolate_field(env, &kv.value, || format!("query `{}`", kv.key))?;
        pairs.push(format!(
            "{}={}",
            percent_encode(&kv.key),
            percent_encode(&value)
        ));
    }
    let joiner = if url.contains('?') { '&' } else { '?' };
    Ok(format!("{url}{joiner}{}", pairs.join("&")))
}

fn prepare_body(body: &Body, env: &Environment) -> Result<PreparedBody, PrepareError> {
    match body {
        Body::None => Ok(PreparedBody::None),
        Body::Text(text) => {
            let value = interpolate_field(env, text, || "body".into())?;
            Ok(PreparedBody::Bytes {
                content_type: "text/plain; charset=utf-8".into(),
                bytes: value.into_bytes(),
            })
        }
        Body::Json(text) => {
            let value = interpolate_field(env, text, || "body".into())?;
            Ok(PreparedBody::Bytes {
                content_type: "application/json".into(),
                bytes: value.into_bytes(),
            })
        }
        Body::Xml(text) => {
            let value = interpolate_field(env, text, || "body".into())?;
            Ok(PreparedBody::Bytes {
                content_type: "application/xml".into(),
                bytes: value.into_bytes(),
            })
        }
        Body::Form(rows) => {
            let mut pairs = Vec::with_capacity(rows.len());
            for kv in rows {
                if kv.disabled {
                    continue;
                }
                let value = interpolate_field(env, &kv.value, || format!("form `{}`", kv.key))?;
                pairs.push((kv.key.clone(), value));
            }
            Ok(PreparedBody::Form(pairs))
        }
    }
}

fn apply_default_content_type(headers: &mut Vec<(String, String)>, body: &PreparedBody) {
    let already_set = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if already_set {
        return;
    }
    let default = match body {
        PreparedBody::None => return,
        PreparedBody::Bytes { content_type, .. } => content_type.clone(),
        PreparedBody::Form(_) => "application/x-www-form-urlencoded".to_string(),
    };
    headers.push(("Content-Type".to_string(), default));
}

fn apply_auth_header(
    headers: &mut Vec<(String, String)>,
    auth: &Auth,
    env: &Environment,
) -> Result<(), PrepareError> {
    let already_set = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    if already_set {
        // Explicit user-supplied Authorization wins; the auth block is
        // ignored. Matches Bruno's "headers override auth block" semantics.
        return Ok(());
    }
    let value = match auth {
        Auth::None => return Ok(()),
        Auth::Basic(basic) => {
            let username =
                interpolate_field(env, &basic.username, || "auth.basic.username".into())?;
            let password =
                interpolate_field(env, &basic.password, || "auth.basic.password".into())?;
            let token = BASE64_STANDARD.encode(format!("{username}:{password}"));
            format!("Basic {token}")
        }
        Auth::Bearer(bearer) => {
            let token = interpolate_field(env, &bearer.token, || "auth.bearer.token".into())?;
            format!("Bearer {token}")
        }
    };
    headers.push(("Authorization".to_string(), value));
    Ok(())
}

/// Minimal percent-encoder for query keys/values.
///
/// Encodes everything outside the RFC 3986 *unreserved* set (`A-Z a-z 0-9
/// - _ . ~`). Good enough for `application/x-www-form-urlencoded`-style
/// query strings and avoids a `percent-encoding` dep for the first cut.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BasicAuth, BearerAuth, HttpMethod, KeyValue};
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

    fn request() -> Request {
        Request::empty()
    }

    #[test]
    fn prepares_a_simple_get_with_interpolated_url() {
        let env = env(&[("base", "https://api.example.com")]);
        let mut req = request();
        req.method = HttpMethod::Get;
        req.url = "{{base}}/users".into();

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(prepared.method, HttpMethod::Get);
        assert_eq!(prepared.url, "https://api.example.com/users");
        assert!(prepared.body.is_none());
        assert!(prepared.headers.is_empty());
    }

    #[test]
    fn interpolates_header_values_and_skips_disabled_rows() {
        let env = env(&[("token", "abc")]);
        let mut req = request();
        req.url = "https://api.example.com".into();
        req.headers = vec![
            KeyValue::new("X-Token", "{{token}}"),
            KeyValue {
                key: "X-Skip".into(),
                value: "nope".into(),
                disabled: true,
            },
        ];

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(
            prepared.headers,
            vec![("X-Token".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn appends_query_parameters_url_encoded() {
        let env = env(&[]);
        let mut req = request();
        req.url = "https://api.example.com/search".into();
        req.query = vec![
            KeyValue::new("q", "hello world"),
            KeyValue::new("filter", "type:user"),
        ];

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(
            prepared.url,
            "https://api.example.com/search?q=hello%20world&filter=type%3Auser"
        );
    }

    #[test]
    fn appends_query_with_ampersand_when_url_already_has_one() {
        let env = env(&[]);
        let mut req = request();
        req.url = "https://api.example.com/search?lang=en".into();
        req.query = vec![KeyValue::new("q", "rust")];

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(
            prepared.url,
            "https://api.example.com/search?lang=en&q=rust"
        );
    }

    #[test]
    fn skips_disabled_query_rows() {
        let env = env(&[]);
        let mut req = request();
        req.url = "https://api.example.com/search".into();
        req.query = vec![
            KeyValue::new("q", "rust"),
            KeyValue {
                key: "draft".into(),
                value: "1".into(),
                disabled: true,
            },
        ];

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(prepared.url, "https://api.example.com/search?q=rust");
    }

    #[test]
    fn json_body_sets_content_type_and_interpolates() {
        let env = env(&[("id", "42")]);
        let mut req = request();
        req.method = HttpMethod::Post;
        req.url = "https://api.example.com/users".into();
        req.body = Body::Json("{\"id\":{{id}}}".into());

        let prepared = prepare_request(&req, &env).unwrap();
        let PreparedBody::Bytes {
            content_type,
            bytes,
        } = prepared.body
        else {
            panic!("expected bytes body");
        };
        assert_eq!(content_type, "application/json");
        assert_eq!(bytes, b"{\"id\":42}");
        assert!(
            prepared
                .headers
                .iter()
                .any(|(k, v)| k == "Content-Type" && v == "application/json")
        );
    }

    #[test]
    fn user_supplied_content_type_wins_over_default() {
        let env = env(&[]);
        let mut req = request();
        req.method = HttpMethod::Post;
        req.url = "https://api.example.com/upload".into();
        req.body = Body::Text("raw payload".into());
        req.headers = vec![KeyValue::new("Content-Type", "application/octet-stream")];

        let prepared = prepare_request(&req, &env).unwrap();
        let content_types: Vec<&str> = prepared
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(content_types, vec!["application/octet-stream"]);
    }

    #[test]
    fn form_body_emits_form_variant_with_default_content_type() {
        let env = env(&[("user", "alice")]);
        let mut req = request();
        req.method = HttpMethod::Post;
        req.url = "https://api.example.com/login".into();
        req.body = Body::Form(vec![
            KeyValue::new("name", "{{user}}"),
            KeyValue {
                key: "skip".into(),
                value: "x".into(),
                disabled: true,
            },
        ]);

        let prepared = prepare_request(&req, &env).unwrap();
        let PreparedBody::Form(pairs) = &prepared.body else {
            panic!("expected form body");
        };
        assert_eq!(pairs, &vec![("name".to_string(), "alice".to_string())]);
        assert!(prepared.headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("content-type") && v == "application/x-www-form-urlencoded"
        }));
    }

    #[test]
    fn bearer_auth_emits_authorization_header() {
        let env = env(&[("token", "secret")]);
        let mut req = request();
        req.url = "https://api.example.com".into();
        req.auth = Auth::Bearer(BearerAuth {
            token: "{{token}}".into(),
        });

        let prepared = prepare_request(&req, &env).unwrap();
        assert_eq!(
            prepared.headers,
            vec![("Authorization".to_string(), "Bearer secret".to_string())]
        );
    }

    #[test]
    fn basic_auth_base64_encodes_credentials() {
        let env = env(&[]);
        let mut req = request();
        req.url = "https://api.example.com".into();
        req.auth = Auth::Basic(BasicAuth {
            username: "alice".into(),
            password: "wonderland".into(),
        });

        let prepared = prepare_request(&req, &env).unwrap();
        // base64("alice:wonderland") = "YWxpY2U6d29uZGVybGFuZA=="
        assert_eq!(
            prepared.headers,
            vec![(
                "Authorization".to_string(),
                "Basic YWxpY2U6d29uZGVybGFuZA==".to_string()
            )]
        );
    }

    #[test]
    fn user_authorization_header_overrides_auth_block() {
        let env = env(&[]);
        let mut req = request();
        req.url = "https://api.example.com".into();
        req.headers = vec![KeyValue::new("Authorization", "Custom xyz")];
        req.auth = Auth::Bearer(BearerAuth {
            token: "should-be-ignored".into(),
        });

        let prepared = prepare_request(&req, &env).unwrap();
        let auth_headers: Vec<&str> = prepared
            .headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(auth_headers, vec!["Custom xyz"]);
    }

    #[test]
    fn empty_url_after_interpolation_is_an_error() {
        let env = env(&[]);
        let mut req = request();
        req.url = "   ".into();

        let err = prepare_request(&req, &env).unwrap_err();
        assert_eq!(err, PrepareError::EmptyUrl);
    }

    #[test]
    fn missing_variable_surfaces_field_context() {
        let env = env(&[]);
        let mut req = request();
        req.url = "{{base}}".into();

        let err = prepare_request(&req, &env).unwrap_err();
        match err {
            PrepareError::Interpolation { field, source } => {
                assert_eq!(field, "url");
                assert_eq!(
                    source,
                    InterpolateError::MissingVariable {
                        name: "base".into()
                    }
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
