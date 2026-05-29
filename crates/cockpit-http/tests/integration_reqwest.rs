//! Integration tests for [`ReqwestEngine`] (v0.11 M11.3.1).
//!
//! Gated behind the `integration` cargo feature so the default
//! `cargo test` stays hermetic and dep-light (per AGENTS §7). Spins up a
//! tiny `TcpListener`-based mock server in-process rather than pulling
//! `wiremock`/tokio just for fixtures — the engine's contract (status,
//! headers, body, cancel) is all we need to assert on, and a hand-rolled
//! mock keeps the test rig observable.
//!
//! Run with:
//!
//! ```bash
//! cargo test -p cockpit-http --features integration --test integration_reqwest
//! ```

#![cfg(feature = "integration")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use cockpit_http::{
    CancelHandle, HttpEngine, HttpError, HttpMethod, PreparedBody, PreparedRequest, ReqwestEngine,
};

/// One scripted HTTP exchange the mock serves before closing the listener.
struct Canned {
    status: u16,
    reason: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// How long to sleep before responding. Lets us provoke timeouts and
    /// cancellation.
    delay: Duration,
}

impl Canned {
    fn new(status: u16, reason: &'static str, body: &[u8]) -> Self {
        Self {
            status,
            reason,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_vec(),
            delay: Duration::ZERO,
        }
    }

    fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// Captured request from the mock — what reqwest actually sent. Only the
/// fields current tests assert on are stored; expand when a new test needs
/// to inspect headers or body.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    path: String,
}

/// A one-shot mock server. Listens on `127.0.0.1:0`, serves the queued
/// `responses` to consecutive connections, and stops accepting once they're
/// all consumed.
struct Mock {
    addr: String,
    captured: Arc<std::sync::Mutex<Vec<Captured>>>,
    served: Arc<AtomicUsize>,
}

impl Mock {
    fn spawn(responses: Vec<Canned>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = format!("http://{}", listener.local_addr().expect("local addr"));
        let captured = Arc::new(std::sync::Mutex::new(Vec::<Captured>::new()));
        let served = Arc::new(AtomicUsize::new(0));

        let captured_for_thread = Arc::clone(&captured);
        let served_for_thread = Arc::clone(&served);
        thread::spawn(move || {
            // Serve up to N responses then drop the listener. Subsequent
            // tests reuse a fresh port.
            for canned in responses {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(_) => return,
                };
                match handle_one(&mut stream, &canned) {
                    Ok(req) => captured_for_thread.lock().unwrap().push(req),
                    Err(_) => return,
                }
                served_for_thread.fetch_add(1, Ordering::SeqCst);
            }
        });

        Self {
            addr,
            captured,
            served,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.addr)
    }

    fn captured(&self) -> Vec<Captured> {
        self.captured.lock().unwrap().clone()
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

fn handle_one(stream: &mut TcpStream, canned: &Canned) -> std::io::Result<Captured> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.trim_end().splitn(3, ' ');
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    // Drain headers; remember Content-Length so we can consume the body
    // before responding (some clients otherwise block on the request side).
    let mut content_length: usize = 0;
    loop {
        let mut header_line = String::new();
        let read = reader.read_line(&mut header_line)?;
        if read == 0 {
            break;
        }
        let trimmed = header_line.trim_end_matches("\r\n");
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    if content_length > 0 {
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
    }

    if !canned.delay.is_zero() {
        thread::sleep(canned.delay);
    }

    let mut response = format!("HTTP/1.1 {} {}\r\n", canned.status, canned.reason);
    for (name, value) in &canned.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n", canned.body.len()));
    response.push_str("Connection: close\r\n\r\n");
    stream.write_all(response.as_bytes())?;
    stream.write_all(&canned.body)?;
    stream.flush()?;

    Ok(Captured { method, path })
}

fn get(url: &str) -> PreparedRequest {
    PreparedRequest {
        method: HttpMethod::Get,
        url: url.into(),
        headers: Vec::new(),
        body: PreparedBody::None,
        timeout: None,
    }
}

#[test]
fn happy_path_get_returns_status_headers_and_body() {
    let mock = Mock::spawn(vec![Canned::new(200, "OK", br#"{"ok":true}"#)]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();

    let response = engine
        .send(get(&mock.url("/health")), &cancel)
        .expect("send");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, br#"{"ok":true}"#);
    assert!(
        response
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v == "application/json")
    );
    assert!(response.elapsed > Duration::ZERO);
    assert!(response.final_url.ends_with("/health"));

    let captured = mock.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, "GET");
    assert_eq!(captured[0].path, "/health");
}

#[test]
fn redirect_chain_follows_to_final_url() {
    let mock = Mock::spawn(vec![
        Canned::new(302, "Found", b"").with_header("Location", "/final"),
        Canned::new(200, "OK", b"arrived"),
    ]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();

    let response = engine
        .send(get(&mock.url("/start")), &cancel)
        .expect("send");

    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"arrived");
    assert!(
        response.final_url.ends_with("/final"),
        "expected final_url to be the redirect target, got {}",
        response.final_url
    );
    assert_eq!(mock.served(), 2);
}

#[test]
fn four_xx_status_round_trips_with_body() {
    let mock = Mock::spawn(vec![Canned::new(
        404,
        "Not Found",
        br#"{"error":"missing"}"#,
    )]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();

    let response = engine.send(get(&mock.url("/who")), &cancel).expect("send");

    assert_eq!(response.status, 404);
    assert_eq!(response.body, br#"{"error":"missing"}"#);
}

#[test]
fn five_xx_status_round_trips_with_body() {
    let mock = Mock::spawn(vec![Canned::new(500, "Internal Server Error", b"boom")]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();

    let response = engine.send(get(&mock.url("/oops")), &cancel).expect("send");

    assert_eq!(response.status, 500);
    assert_eq!(response.body, b"boom");
}

#[test]
fn per_request_timeout_surfaces_as_timeout_error() {
    let mock = Mock::spawn(vec![
        Canned::new(200, "OK", b"slow").with_delay(Duration::from_millis(500)),
    ]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();
    let mut req = get(&mock.url("/slow"));
    req.timeout = Some(Duration::from_millis(75));

    let err = engine.send(req, &cancel).unwrap_err();

    assert!(matches!(err, HttpError::Timeout(_)), "got {err:?}");
}

#[test]
fn network_error_when_target_is_unreachable() {
    // Bind+drop to get a port we know nothing is listening on. There's an
    // unavoidable race here (the OS could re-assign), but it's vanishingly
    // unlikely inside a single test process.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    let url = format!("http://{addr}/unreachable");

    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();

    let err = engine.send(get(&url), &cancel).unwrap_err();
    assert!(
        matches!(err, HttpError::Network(_)),
        "expected Network, got {err:?}"
    );
}

#[test]
fn cancel_handle_interrupts_in_flight_request() {
    let mock = Mock::spawn(vec![
        Canned::new(200, "OK", b"never-arrives").with_delay(Duration::from_secs(5)),
    ]);
    let engine = ReqwestEngine::new().expect("engine");
    let cancel = CancelHandle::new();
    let cancel_handle = cancel.clone();

    // Trip cancel after the request has had a chance to land at the mock.
    let (started_tx, started_rx) = mpsc::channel();
    thread::spawn(move || {
        // Best-effort: wait briefly so reqwest is actually mid-call.
        thread::sleep(Duration::from_millis(150));
        started_tx.send(()).ok();
        cancel_handle.cancel();
    });

    let start = Instant::now();
    let err = engine.send(get(&mock.url("/slow")), &cancel).unwrap_err();
    let elapsed = start.elapsed();

    // The canceller thread should have run by now; if not, the test setup
    // is broken and the assertion below would be meaningless.
    started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("canceller fired");

    assert_eq!(err, HttpError::Cancelled);
    // Cancel poll is 50 ms, schedule slop a few hundred ms: well under the
    // 5 s the mock would otherwise hold us for.
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel didn't interrupt promptly: {elapsed:?}"
    );
}
