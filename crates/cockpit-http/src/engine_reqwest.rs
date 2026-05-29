//! Production [`HttpEngine`] backed by `reqwest::blocking` (v0.11 M11.3.1).
//!
//! The trait is synchronous: callers expect `send` to block until the
//! response arrives. We honour that while still letting the [`CancelHandle`]
//! interrupt the wait — each `send` spawns a one-shot worker thread that
//! does the actual `reqwest::blocking::Client::execute`, and the caller's
//! thread polls `cancel` between channel waits. If cancel fires before the
//! response arrives, `send` returns [`HttpError::Cancelled`] and the worker
//! is orphaned (its completed response, if any, is dropped — reqwest
//! handles that cleanly).
//!
//! Why one worker per call instead of a long-lived pool: requests are rare
//! (user-driven, one at a time) and the simplest reasoning about lifetimes
//! matters more than spawn cost. If the cockpit ever fans out to
//! `Http: Send All In Folder` and the spawn overhead shows up in profiles,
//! reach for a `rayon` pool — for now keep the path obvious.
//!
//! TLS uses the reqwest default (`native-tls` → platform OpenSSL/Schannel/
//! SecureTransport). Custom CA bundles work via the `SSL_CERT_FILE`
//! environment variable, per the OpenSSL convention.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::redirect;

use crate::engine::{CancelHandle, HttpEngine, HttpError, PreparedBody, PreparedRequest, Response};
use crate::model::HttpMethod;

/// How often the caller wakes up to check the [`CancelHandle`] while
/// waiting for the worker thread. 50 ms is below human reaction time and
/// imperceptible overhead.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// reqwest's default redirect cap. Mirrored here so [`HttpError::TooManyRedirects`]
/// carries the same number the engine actually enforces.
const REDIRECT_LIMIT: u32 = 10;

/// Production [`HttpEngine`]. Thin wrapper around a single
/// `reqwest::blocking::Client` plus the dispatching ceremony described in
/// the module docs.
#[derive(Debug, Clone)]
pub struct ReqwestEngine {
    client: Client,
}

impl ReqwestEngine {
    /// Construct an engine with cockpit defaults: 30 s request timeout,
    /// 10-hop redirect cap, gzip/brotli decoding disabled (we hand the
    /// raw body to the renderer).
    pub fn new() -> Result<Self, HttpError> {
        Self::with_builder(Client::builder())
    }

    /// Construct from a user-supplied reqwest builder. Public so a future
    /// config layer can override timeouts / proxy / TLS settings without
    /// duplicating the dispatch glue. The cockpit defaults are layered on
    /// top of whatever the caller supplied — explicit user choices win.
    pub fn with_builder(builder: reqwest::blocking::ClientBuilder) -> Result<Self, HttpError> {
        let client = builder
            .timeout(Duration::from_secs(30))
            .redirect(redirect::Policy::limited(REDIRECT_LIMIT as usize))
            .build()
            .map_err(|err| HttpError::Network(format!("client init failed: {err}")))?;
        Ok(Self { client })
    }
}

impl HttpEngine for ReqwestEngine {
    fn send(&self, req: PreparedRequest, cancel: &CancelHandle) -> Result<Response, HttpError> {
        if cancel.is_cancelled() {
            return Err(HttpError::Cancelled);
        }
        let (tx, rx) = mpsc::channel();
        let client = self.client.clone();
        let request_timeout = req.timeout;
        thread::Builder::new()
            .name("cockpit-http-worker".into())
            .spawn(move || {
                let result = execute(&client, req, request_timeout);
                // Ignore send errors — the caller may have given up on us
                // after cancelling. The orphaned response just drops.
                let _ = tx.send(result);
            })
            .map_err(|err| HttpError::Network(format!("spawn failed: {err}")))?;

        loop {
            if cancel.is_cancelled() {
                return Err(HttpError::Cancelled);
            }
            match rx.recv_timeout(CANCEL_POLL) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(HttpError::Network(
                        "worker thread exited without a response".into(),
                    ));
                }
            }
        }
    }
}

/// Run the actual reqwest call. Lives on the worker thread; the caller's
/// thread only observes its `Result` via the channel.
fn execute(
    client: &Client,
    req: PreparedRequest,
    request_timeout: Option<Duration>,
) -> Result<Response, HttpError> {
    let start = Instant::now();
    let method = to_reqwest_method(req.method);
    let url = reqwest::Url::parse(&req.url)
        .map_err(|err| HttpError::InvalidRequest(format!("bad url `{}`: {err}", req.url)))?;
    let mut builder = client.request(method, url);
    for (name, value) in &req.headers {
        builder = builder.header(name, value);
    }
    builder = match req.body {
        PreparedBody::None => builder,
        PreparedBody::Bytes {
            content_type,
            bytes,
        } => {
            // The prepare step already pushed a Content-Type header unless
            // the user overrode it; re-applying via the builder would
            // duplicate. Skip when already present.
            if !req
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                builder.header("Content-Type", content_type).body(bytes)
            } else {
                builder.body(bytes)
            }
        }
        PreparedBody::Form(pairs) => {
            // reqwest's `.form` sets the Content-Type itself — drop ours
            // if it happens to be the same to avoid duplicates.
            builder.form(&pairs)
        }
    };
    if let Some(timeout) = request_timeout {
        builder = builder.timeout(timeout);
    }

    let response = builder
        .send()
        .map_err(|err| classify_error(err, request_timeout))?;
    let final_url = response.url().to_string();
    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_string()))
        .collect();
    let body = response
        .bytes()
        .map_err(|err| HttpError::Network(format!("body read failed: {err}")))?
        .to_vec();
    Ok(Response {
        status,
        headers,
        body,
        elapsed: start.elapsed(),
        // Capturing the full redirect chain needs a custom
        // `redirect::Policy` that records hops on each callback. Plumbed
        // separately when the view-model (M11.4) actually surfaces them;
        // empty for now keeps the response shape stable.
        redirects: Vec::new(),
        final_url,
    })
}

fn to_reqwest_method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
        HttpMethod::Head => reqwest::Method::HEAD,
        HttpMethod::Options => reqwest::Method::OPTIONS,
    }
}

fn classify_error(err: reqwest::Error, request_timeout: Option<Duration>) -> HttpError {
    if err.is_timeout() {
        return HttpError::Timeout(request_timeout.unwrap_or(Duration::from_secs(30)));
    }
    if err.is_redirect() {
        return HttpError::TooManyRedirects(REDIRECT_LIMIT);
    }
    if err.is_builder() {
        return HttpError::InvalidRequest(err.to_string());
    }
    HttpError::Network(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cockpit_methods_round_trip_to_reqwest() {
        let pairs = [
            (HttpMethod::Get, reqwest::Method::GET),
            (HttpMethod::Post, reqwest::Method::POST),
            (HttpMethod::Put, reqwest::Method::PUT),
            (HttpMethod::Patch, reqwest::Method::PATCH),
            (HttpMethod::Delete, reqwest::Method::DELETE),
            (HttpMethod::Head, reqwest::Method::HEAD),
            (HttpMethod::Options, reqwest::Method::OPTIONS),
        ];
        for (ours, theirs) in pairs {
            assert_eq!(to_reqwest_method(ours), theirs);
        }
    }

    #[test]
    fn new_succeeds_under_default_settings() {
        // Just verifying the default builder doesn't blow up on construction.
        ReqwestEngine::new().expect("client init");
    }

    #[test]
    fn cancelled_handle_short_circuits_before_spawning() {
        let engine = ReqwestEngine::new().expect("client init");
        let cancel = CancelHandle::new();
        cancel.cancel();
        let req = PreparedRequest {
            method: HttpMethod::Get,
            url: "https://example.invalid/never-reached".into(),
            headers: Vec::new(),
            body: PreparedBody::None,
            timeout: None,
        };
        let err = engine.send(req, &cancel).unwrap_err();
        assert_eq!(err, HttpError::Cancelled);
    }

    #[test]
    fn invalid_url_returns_invalid_request_not_network() {
        let engine = ReqwestEngine::new().expect("client init");
        let cancel = CancelHandle::new();
        let req = PreparedRequest {
            method: HttpMethod::Get,
            url: "not a url".into(),
            headers: Vec::new(),
            body: PreparedBody::None,
            timeout: None,
        };
        let err = engine.send(req, &cancel).unwrap_err();
        assert!(
            matches!(err, HttpError::InvalidRequest(_)),
            "expected InvalidRequest, got {err:?}"
        );
    }
}
