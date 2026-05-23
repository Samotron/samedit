//! Process-managed LSP client.
//!
//! Spawns a server binary, runs blocking reader/writer threads against its
//! stdio (no async runtime — AGENTS rule #4), and exposes a non-blocking
//! `notify` / `request` / `try_recv` surface for the app. Outgoing messages
//! queue through a channel so the caller never waits on the wire, and
//! incoming messages drain into another channel for the app to poll.

use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::codec::{self, CodecError};
use crate::jsonrpc::{self, JsonRpcError, Notification, Request, Response};

/// One incoming message from the language server, fanned out by the reader
/// thread for the app to consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecvMessage {
    /// Response to a request we sent.
    Response(Response),
    /// Server-initiated request — the app must reply with a result.
    ServerRequest {
        id: i64,
        method: String,
        params: Value,
    },
    /// Server-initiated notification — no reply expected.
    ServerNotification { method: String, params: Value },
    /// Decode failed — message preserved for diagnostics.
    Decode { error: String, raw: Vec<u8> },
}

/// A spawned LSP client. Drop the value to terminate the server.
pub struct LspClient {
    child: Child,
    outbound_tx: Option<Sender<Vec<u8>>>,
    inbound_rx: Receiver<RecvMessage>,
    reader_thread: Option<JoinHandle<()>>,
    writer_thread: Option<JoinHandle<()>>,
    next_id: AtomicI64,
}

impl LspClient {
    /// Spawn `program` with `args` and wire its stdio to background threads.
    /// `working_dir`, when supplied, becomes the child's `cwd`.
    pub fn spawn(
        program: &str,
        args: &[String],
        working_dir: Option<&Path>,
    ) -> Result<Self, ClientError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = working_dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn().map_err(ClientError::Spawn)?;
        let stdin = child.stdin.take().ok_or(ClientError::PipeMissing)?;
        let stdout = child.stdout.take().ok_or(ClientError::PipeMissing)?;

        let (outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>();
        let (inbound_tx, inbound_rx) = mpsc::channel::<RecvMessage>();

        let writer_thread = thread::Builder::new()
            .name("cockpit-lsp-writer".to_string())
            .spawn(move || writer_loop(stdin, outbound_rx))
            .map_err(ClientError::ThreadSpawn)?;
        let reader_thread = thread::Builder::new()
            .name("cockpit-lsp-reader".to_string())
            .spawn(move || reader_loop(BufReader::new(stdout), inbound_tx))
            .map_err(ClientError::ThreadSpawn)?;

        Ok(Self {
            child,
            outbound_tx: Some(outbound_tx),
            inbound_rx,
            reader_thread: Some(reader_thread),
            writer_thread: Some(writer_thread),
            next_id: AtomicI64::new(1),
        })
    }

    /// Queue a JSON-RPC notification. Never blocks on the wire.
    pub fn notify<P: Serialize>(&self, method: &str, params: P) -> Result<(), ClientError> {
        let notification = Notification::new(method, params);
        let bytes = jsonrpc::encode_notification(&notification).map_err(ClientError::Encode)?;
        self.enqueue(bytes)
    }

    /// Queue a JSON-RPC request and return the assigned id. Pair the id with
    /// the matching [`RecvMessage::Response`] yielded by [`try_recv`].
    pub fn request<P: Serialize>(&self, method: &str, params: P) -> Result<i64, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = Request::new(id, method, params);
        let bytes = jsonrpc::encode_request(&request).map_err(ClientError::Encode)?;
        self.enqueue(bytes)?;
        Ok(id)
    }

    /// Non-blocking poll for the next inbound message.
    pub fn try_recv(&self) -> Option<RecvMessage> {
        self.inbound_rx.try_recv().ok()
    }

    fn enqueue(&self, bytes: Vec<u8>) -> Result<(), ClientError> {
        let tx = self.outbound_tx.as_ref().ok_or(ClientError::Closed)?;
        tx.send(bytes).map_err(|_| ClientError::Closed)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        // Closing the outbound channel signals the writer thread to exit,
        // which drops the child's stdin and EOFs the server.
        self.outbound_tx.take();
        let _ = self.child.kill();
        if let Some(thread) = self.writer_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        let _ = self.child.wait();
    }
}

fn writer_loop<W: Write>(mut writer: W, outbound: Receiver<Vec<u8>>) {
    while let Ok(payload) = outbound.recv() {
        if let Err(err) = codec::write_message(&mut writer, &payload) {
            tracing::warn!(?err, "LSP writer failed; ending thread");
            break;
        }
    }
}

fn reader_loop<R: std::io::BufRead>(mut reader: R, inbound: Sender<RecvMessage>) {
    loop {
        match codec::read_message(&mut reader) {
            Ok(payload) => {
                let message = classify(&payload);
                if inbound.send(message).is_err() {
                    break;
                }
            }
            Err(CodecError::Eof) => break,
            Err(err) => {
                let _ = inbound.send(RecvMessage::Decode {
                    error: err.to_string(),
                    raw: Vec::new(),
                });
                break;
            }
        }
    }
}

fn classify(bytes: &[u8]) -> RecvMessage {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return RecvMessage::Decode {
            error: "payload was not valid JSON".to_string(),
            raw: bytes.to_vec(),
        };
    };
    let has_id = value.get("id").is_some_and(|id| !id.is_null());
    let has_method = value.get("method").is_some();
    match (has_id, has_method) {
        // Response to a request we sent.
        (true, false) => match jsonrpc::decode_response(bytes) {
            Ok(response) => RecvMessage::Response(response),
            Err(err) => RecvMessage::Decode {
                error: err.to_string(),
                raw: bytes.to_vec(),
            },
        },
        // Server-initiated request.
        (true, true) => RecvMessage::ServerRequest {
            id: value.get("id").and_then(Value::as_i64).unwrap_or(0),
            method: value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        },
        // Server-initiated notification.
        (false, true) => RecvMessage::ServerNotification {
            method: value
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            params: value.get("params").cloned().unwrap_or(Value::Null),
        },
        // Neither — undecodable in the JSON-RPC sense.
        (false, false) => RecvMessage::Decode {
            error: "payload is neither a request, notification, nor response".to_string(),
            raw: bytes.to_vec(),
        },
    }
}

/// LSP client error.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to spawn language server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("failed to spawn a worker thread: {0}")]
    ThreadSpawn(#[source] std::io::Error),
    #[error("language server stdio pipe was missing")]
    PipeMissing,
    #[error("failed to encode outgoing message: {0}")]
    Encode(#[source] JsonRpcError),
    #[error("client transport is closed")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognises_a_response() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}"#;
        match classify(bytes) {
            RecvMessage::Response(response) => assert_eq!(response.id, Some(1)),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classify_recognises_a_server_notification() {
        let bytes =
            br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"x"}}"#;
        match classify(bytes) {
            RecvMessage::ServerNotification { method, .. } => {
                assert_eq!(method, "window/logMessage");
            }
            other => panic!("expected ServerNotification, got {other:?}"),
        }
    }

    #[test]
    fn classify_recognises_a_server_request() {
        let bytes =
            br#"{"jsonrpc":"2.0","id":42,"method":"client/registerCapability","params":{}}"#;
        match classify(bytes) {
            RecvMessage::ServerRequest { id, method, .. } => {
                assert_eq!(id, 42);
                assert_eq!(method, "client/registerCapability");
            }
            other => panic!("expected ServerRequest, got {other:?}"),
        }
    }

    #[test]
    fn classify_reports_decode_errors_for_garbage() {
        let bytes = b"not json at all";
        match classify(bytes) {
            RecvMessage::Decode { error, raw } => {
                assert!(!error.is_empty());
                assert_eq!(raw, bytes);
            }
            other => panic!("expected Decode, got {other:?}"),
        }
    }

    #[cfg(feature = "integration")]
    #[test]
    fn spawning_a_pipe_through_cat_round_trips_a_message() {
        use std::time::{Duration, Instant};

        // `cat` echoes whatever cockpit writes back as if it were a server
        // reply. It is enough to exercise the spawn → write → read cycle
        // without depending on a real language server.
        let program = if cfg!(windows) { "cmd.exe" } else { "cat" };
        let args: Vec<String> = if cfg!(windows) {
            // Windows cmd echoes stdin via `findstr /R .*`. Skip on Windows
            // for the foundation; the integration leg targets Unix shells.
            return;
        } else {
            Vec::new()
        };

        let client = LspClient::spawn(program, &args, None).unwrap();
        client
            .notify("ping", serde_json::json!({"hello": "world"}))
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut got_back = None;
        while Instant::now() < deadline {
            if let Some(message) = client.try_recv() {
                got_back = Some(message);
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(got_back, Some(RecvMessage::ServerNotification { ref method, .. }) if method == "ping"),
            "expected the framed notification to round-trip through cat: {got_back:?}",
        );
    }
}
