//! `HttpEngine` trait + scriptable fake (v0.11 M11.3).
//!
//! The engine sits between the view-model and the network. Callers hand it
//! a [`PreparedRequest`] (already interpolated, no `{{vars}}` left) and a
//! [`CancelHandle`]; the engine returns a [`Response`] or an [`HttpError`].
//! All entry points are synchronous on the caller's perspective — the
//! `reqwest::blocking` worker thread lives behind the trait (lands in a
//! follow-up). Tests use [`FakeHttpEngine`], which records the requests it
//! received and replays scripted responses.
//!
//! No `tokio`, no `async`, no `winit` — AGENTS §2 #2 and #4.
//!
//! # Cancellation
//!
//! [`CancelHandle`] wraps an `Arc<AtomicBool>`. The UI thread keeps a clone
//! around for as long as a request is in flight and trips it on `Ctrl-C`.
//! The engine polls the handle between syscalls; the fake honours it
//! immediately.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::HttpMethod;

/// A request ready to leave the engine's I/O thread.
///
/// All `{{var}}` interpolation, Lua scripting (M11.6), and auth-header
/// derivation has happened upstream — the engine only translates this
/// into a single network call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRequest {
    pub method: HttpMethod,
    /// Final URL including any encoded query string.
    pub url: String,
    /// Headers in declaration order. Duplicates allowed (HTTP permits them).
    pub headers: Vec<(String, String)>,
    pub body: PreparedBody,
    /// Optional per-request timeout. `None` means "use the engine default".
    pub timeout: Option<Duration>,
}

/// The body of a [`PreparedRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreparedBody {
    None,
    /// A raw byte body plus its `Content-Type`. JSON, XML, and arbitrary
    /// `text/plain` payloads all flow through here.
    Bytes {
        content_type: String,
        bytes: Vec<u8>,
    },
    /// `application/x-www-form-urlencoded` — emitted form-urlencoded by the
    /// engine. Kept structured rather than pre-serialised so the engine can
    /// pick the encoder it ships with.
    Form(Vec<(String, String)>),
}

impl PreparedBody {
    pub fn is_none(&self) -> bool {
        matches!(self, PreparedBody::None)
    }
}

/// A response delivered back to the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Total wall-clock duration spent in the engine — from
    /// `send` to last byte read. Excludes serialisation on either end.
    pub elapsed: Duration,
    /// Redirect hops followed before reaching `final_url`. Empty for direct
    /// responses.
    pub redirects: Vec<RedirectHop>,
    /// URL of the response (post-redirect).
    pub final_url: String,
}

/// One step of a redirect chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedirectHop {
    pub from: String,
    pub to: String,
    pub status: u16,
}

/// Cooperative cancellation handle shared between the UI and engine threads.
///
/// `Arc<AtomicBool>` under the hood, so cloning is cheap and concurrent
/// readers (the engine polling, the UI tripping) don't fight. The engine
/// is expected to check `is_cancelled` between syscalls; the
/// [`FakeHttpEngine`] checks it before returning each scripted response.
#[derive(Debug, Clone, Default)]
pub struct CancelHandle {
    flag: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trip the handle. Subsequent `is_cancelled` calls return `true`.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

/// Engine-side failure modes. Distinct from parse/prepare errors —
/// callers handle those before reaching the engine.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HttpError {
    #[error("request timed out after {0:?}")]
    Timeout(Duration),
    #[error("request cancelled by user")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
    #[error("TLS error: {0}")]
    Tls(String),
    /// The prepared request didn't make sense to the engine (e.g. malformed
    /// URL, header value containing control bytes). Distinct from
    /// `PrepareError` — those fire at interpolation time.
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    /// Too many redirects, redirect chain longer than the engine's limit.
    #[error("redirect chain exceeded {0} hops")]
    TooManyRedirects(u32),
}

/// The single engine seam — implement this for the production
/// `reqwest::blocking` worker and the in-process [`FakeHttpEngine`].
pub trait HttpEngine: Send + Sync {
    /// Send `req`, blocking until a response arrives or `cancel` is tripped.
    fn send(&self, req: PreparedRequest, cancel: &CancelHandle) -> Result<Response, HttpError>;
}

/// Scripted [`HttpEngine`] for headless tests.
///
/// Push exchanges with [`push_response`] or [`push_error`]; calls to
/// `send` consume them in order. Inspect `requests()` afterwards to assert
/// on what the engine saw.
#[derive(Debug, Default)]
pub struct FakeHttpEngine {
    queued: Mutex<VecDeque<Scripted>>,
    seen: Mutex<Vec<PreparedRequest>>,
}

#[derive(Debug)]
enum Scripted {
    Response(Response),
    Error(HttpError),
}

impl FakeHttpEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue the next call's response.
    pub fn push_response(&self, response: Response) {
        self.queued
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .push_back(Scripted::Response(response));
    }

    /// Queue the next call's error.
    pub fn push_error(&self, error: HttpError) {
        self.queued
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .push_back(Scripted::Error(error));
    }

    /// Snapshot the requests the engine has seen, in call order.
    pub fn requests(&self) -> Vec<PreparedRequest> {
        self.seen
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .clone()
    }

    /// Number of pending scripted exchanges. Useful to assert "engine was
    /// drained" at the end of a test.
    pub fn pending(&self) -> usize {
        self.queued
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .len()
    }
}

impl HttpEngine for FakeHttpEngine {
    fn send(&self, req: PreparedRequest, cancel: &CancelHandle) -> Result<Response, HttpError> {
        if cancel.is_cancelled() {
            return Err(HttpError::Cancelled);
        }
        self.seen
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .push(req);
        let next = self
            .queued
            .lock()
            .expect("FakeHttpEngine mutex poisoned")
            .pop_front();
        match next {
            Some(Scripted::Response(resp)) => Ok(resp),
            Some(Scripted::Error(err)) => Err(err),
            None => Err(HttpError::Network(
                "FakeHttpEngine ran out of scripted exchanges".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_response() -> Response {
        Response {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: b"{}".to_vec(),
            elapsed: Duration::from_millis(12),
            redirects: Vec::new(),
            final_url: "https://api.example.com".into(),
        }
    }

    fn get_request(url: &str) -> PreparedRequest {
        PreparedRequest {
            method: HttpMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: PreparedBody::None,
            timeout: None,
        }
    }

    #[test]
    fn fake_engine_returns_queued_response_and_records_request() {
        let engine = FakeHttpEngine::new();
        engine.push_response(ok_response());
        let cancel = CancelHandle::new();

        let response = engine
            .send(get_request("https://api.example.com"), &cancel)
            .expect("send");

        assert_eq!(response.status, 200);
        assert_eq!(engine.requests().len(), 1);
        assert_eq!(engine.requests()[0].url, "https://api.example.com");
        assert_eq!(engine.pending(), 0);
    }

    #[test]
    fn fake_engine_replays_in_fifo_order() {
        let engine = FakeHttpEngine::new();
        engine.push_response(Response {
            status: 200,
            ..ok_response()
        });
        engine.push_response(Response {
            status: 201,
            ..ok_response()
        });
        let cancel = CancelHandle::new();

        let first = engine.send(get_request("https://a"), &cancel).unwrap();
        let second = engine.send(get_request("https://b"), &cancel).unwrap();

        assert_eq!(first.status, 200);
        assert_eq!(second.status, 201);
        assert_eq!(engine.requests().len(), 2);
    }

    #[test]
    fn fake_engine_returns_scripted_errors() {
        let engine = FakeHttpEngine::new();
        engine.push_error(HttpError::Timeout(Duration::from_secs(5)));
        let cancel = CancelHandle::new();

        let err = engine
            .send(get_request("https://slow"), &cancel)
            .unwrap_err();

        assert_eq!(err, HttpError::Timeout(Duration::from_secs(5)));
    }

    #[test]
    fn fake_engine_returns_cancelled_when_handle_is_tripped() {
        let engine = FakeHttpEngine::new();
        engine.push_response(ok_response());
        let cancel = CancelHandle::new();
        cancel.cancel();

        let err = engine
            .send(get_request("https://api.example.com"), &cancel)
            .unwrap_err();

        assert_eq!(err, HttpError::Cancelled);
        // Cancellation short-circuits before the request is recorded.
        assert!(engine.requests().is_empty());
        // …and before the queued response is consumed.
        assert_eq!(engine.pending(), 1);
    }

    #[test]
    fn fake_engine_surfaces_underflow_as_a_clear_error() {
        let engine = FakeHttpEngine::new();
        let cancel = CancelHandle::new();

        let err = engine.send(get_request("https://x"), &cancel).unwrap_err();

        assert!(matches!(err, HttpError::Network(_)), "{err:?}");
    }

    #[test]
    fn cancel_handle_clones_share_state() {
        let a = CancelHandle::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }
}
