//! `cockpit-lsp` — headless LSP client foundation.
//!
//! This crate owns the v0.3 LSP groundwork: JSON-RPC/LSP message framing,
//! stdio process plumbing on a dedicated reader thread, server command
//! planning, and lazy-start policy. Feature surfaces such as diagnostics and
//! go-to-definition land later and consume this crate.

use std::{
    io::{self, BufRead, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use lsp_types::{notification::Notification, request::Request};
use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;

/// Files larger than this do not auto-start an LSP server.
pub const DEFAULT_MAX_LSP_FILE_BYTES: u64 = 1_000_000;

/// One language Cockpit knows how to map to a language-server command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspLanguage {
    Rust,
    Python,
    Zig,
    TypeScript,
    JavaScript,
}

impl LspLanguage {
    /// Infer an LSP language from a file path.
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        let path = path.as_ref();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("rs") => Some(Self::Rust),
            Some("py") => Some(Self::Python),
            Some("zig") => Some(Self::Zig),
            Some("ts") | Some("tsx") => Some(Self::TypeScript),
            Some("js") | Some("jsx") => Some(Self::JavaScript),
            _ => None,
        }
    }
}

/// Command used to start a language server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspServerCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl LspServerCommand {
    pub fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// Build a server command for a language. When `use_mise` is true, the command
/// is wrapped with `mise exec --` so the server inherits the project tool env.
pub fn server_command(language: LspLanguage, use_mise: bool) -> LspServerCommand {
    let base = match language {
        LspLanguage::Rust => LspServerCommand::new("rust-analyzer", Vec::<String>::new()),
        LspLanguage::Python => LspServerCommand::new("pyright-langserver", ["--stdio"]),
        LspLanguage::Zig => LspServerCommand::new("zls", Vec::<String>::new()),
        LspLanguage::TypeScript | LspLanguage::JavaScript => {
            LspServerCommand::new("typescript-language-server", ["--stdio"])
        }
    };

    if !use_mise {
        return base;
    }

    let mut args = vec!["exec".to_string(), "--".to_string(), base.program];
    args.extend(base.args);
    LspServerCommand::new("mise", args)
}

/// A document considered for lazy LSP startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocument {
    pub path: std::path::PathBuf,
    pub file_size_bytes: u64,
}

/// Lazy-start policy from spec §19.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LspStartPolicy {
    pub max_file_bytes: u64,
}

impl Default for LspStartPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_LSP_FILE_BYTES,
        }
    }
}

impl LspStartPolicy {
    /// Return the language that should start, or `None` when startup should be
    /// skipped for this document.
    pub fn language_to_start(&self, document: &LspDocument) -> Option<LspLanguage> {
        if document.file_size_bytes > self.max_file_bytes {
            return None;
        }
        LspLanguage::from_path(&document.path)
    }
}

/// Events emitted by the LSP reader thread.
#[derive(Debug, PartialEq)]
pub enum LspEvent {
    Message(Value),
    Eof,
    ReadError(String),
}

/// Handle for a spawned LSP process.
pub struct LspClient {
    stdin: Arc<Mutex<ChildStdin>>,
    receiver: Receiver<LspEvent>,
    reader: JoinHandle<()>,
    child: Child,
    next_id: AtomicU64,
}

impl LspClient {
    /// Spawn a language server and move stdout reading onto a dedicated thread.
    pub fn spawn(command: &LspServerCommand, root: impl AsRef<Path>) -> Result<Self, LspError> {
        let mut child = Command::new(&command.program)
            .args(&command.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(LspError::Spawn)?;

        let stdin = child.stdin.take().ok_or(LspError::MissingPipe("stdin"))?;
        let stdout = child.stdout.take().ok_or(LspError::MissingPipe("stdout"))?;
        let (sender, receiver) = mpsc::channel();
        let reader = thread::spawn(move || read_loop(io::BufReader::new(stdout), sender));

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            receiver,
            reader,
            child,
            next_id: AtomicU64::new(1),
        })
    }

    /// Receive the next reader-thread event.
    pub fn recv(&self) -> Result<LspEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Send an arbitrary JSON-RPC message.
    pub fn send_message(&self, message: &Value) -> Result<(), LspError> {
        let bytes = encode_message(message)?;
        let mut stdin = self.stdin.lock().map_err(|_| LspError::WriterPoisoned)?;
        stdin.write_all(&bytes).map_err(LspError::Write)?;
        stdin.flush().map_err(LspError::Write)
    }

    /// Send a typed LSP request and return its JSON-RPC id.
    pub fn send_request<R>(&self, params: R::Params) -> Result<u64, LspError>
    where
        R: Request,
        R::Params: Serialize,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": R::METHOD,
            "params": params,
        });
        self.send_message(&message)?;
        Ok(id)
    }

    /// Send a typed LSP notification.
    pub fn send_notification<N>(&self, params: N::Params) -> Result<(), LspError>
    where
        N: Notification,
        N::Params: Serialize,
    {
        let message = json!({
            "jsonrpc": "2.0",
            "method": N::METHOD,
            "params": params,
        });
        self.send_message(&message)
    }

    /// Ask the server process to terminate and join the reader thread.
    pub fn shutdown(mut self) -> Result<(), LspError> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reader.join().map_err(|_| LspError::ReaderPanicked)
    }
}

fn read_loop(mut reader: impl BufRead, sender: Sender<LspEvent>) {
    loop {
        match read_message(&mut reader) {
            Ok(Some(message)) => {
                if sender.send(LspEvent::Message(message)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = sender.send(LspEvent::Eof);
                break;
            }
            Err(err) => {
                let _ = sender.send(LspEvent::ReadError(err.to_string()));
                break;
            }
        }
    }
}

/// Encode one JSON-RPC/LSP message with `Content-Length` framing.
pub fn encode_message(message: &Value) -> Result<Vec<u8>, LspError> {
    let body = serde_json::to_vec(message).map_err(LspError::Json)?;
    let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    framed.extend(body);
    Ok(framed)
}

/// Read one JSON-RPC/LSP message. Returns `Ok(None)` on clean EOF before a
/// header begins.
pub fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, LspError> {
    let Some(length) = read_content_length(reader)? else {
        return Ok(None);
    };

    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(LspError::Read)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(LspError::Json)
}

fn read_content_length(reader: &mut impl BufRead) -> Result<Option<usize>, LspError> {
    let mut length = None;
    let mut saw_header = false;

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).map_err(LspError::Read)?;
        if bytes == 0 {
            return if saw_header {
                Err(LspError::Protocol(
                    "unexpected EOF in LSP headers".to_string(),
                ))
            } else {
                Ok(None)
            };
        }

        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            return length
                .map(Some)
                .ok_or_else(|| LspError::Protocol("missing Content-Length header".to_string()));
        }
        saw_header = true;

        let Some((name, value)) = trimmed.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| LspError::Protocol("invalid Content-Length".to_string()))?,
            );
        }
    }
}

/// LSP foundation errors.
#[derive(Debug, Error)]
pub enum LspError {
    #[error("failed to spawn language server: {0}")]
    Spawn(#[source] io::Error),
    #[error("language server did not expose {0}")]
    MissingPipe(&'static str),
    #[error("failed to read LSP message: {0}")]
    Read(#[source] io::Error),
    #[error("failed to write LSP message: {0}")]
    Write(#[source] io::Error),
    #[error("failed to encode or decode JSON-RPC: {0}")]
    Json(#[source] serde_json::Error),
    #[error("LSP protocol error: {0}")]
    Protocol(String),
    #[error("LSP writer lock is poisoned")]
    WriterPoisoned,
    #[error("LSP reader thread panicked")]
    ReaderPanicked,
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, path::PathBuf};

    use lsp_types::request::Initialize;

    use super::*;

    #[test]
    fn command_planning_wraps_servers_with_mise_exec() {
        assert_eq!(
            server_command(LspLanguage::Rust, true),
            LspServerCommand::new("mise", ["exec", "--", "rust-analyzer"])
        );
        assert_eq!(
            server_command(LspLanguage::Python, true),
            LspServerCommand::new("mise", ["exec", "--", "pyright-langserver", "--stdio"])
        );
        assert_eq!(
            server_command(LspLanguage::TypeScript, false),
            LspServerCommand::new("typescript-language-server", ["--stdio"])
        );
    }

    #[test]
    fn lazy_policy_starts_only_known_small_documents() {
        let policy = LspStartPolicy { max_file_bytes: 10 };
        assert_eq!(
            policy.language_to_start(&LspDocument {
                path: PathBuf::from("src/main.rs"),
                file_size_bytes: 10,
            }),
            Some(LspLanguage::Rust)
        );
        assert_eq!(
            policy.language_to_start(&LspDocument {
                path: PathBuf::from("src/main.rs"),
                file_size_bytes: 11,
            }),
            None
        );
        assert_eq!(
            policy.language_to_start(&LspDocument {
                path: PathBuf::from("README.md"),
                file_size_bytes: 1,
            }),
            None
        );
    }

    #[test]
    fn message_codec_round_trips_json_rpc() {
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {}
        });
        let bytes = encode_message(&message).unwrap();
        let decoded = read_message(&mut Cursor::new(bytes)).unwrap();
        assert_eq!(decoded, Some(message));
    }

    #[test]
    fn message_reader_accepts_extra_headers_and_multiple_messages() {
        let body = br#"{"jsonrpc":"2.0"}"#;
        let mut input = format!(
            "Content-Length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\n\r\n",
            body.len()
        )
        .into_bytes();
        input.extend(body);
        input.extend(encode_message(&json!({ "id": 2 })).unwrap());
        let mut reader = Cursor::new(input);
        assert_eq!(
            read_message(&mut reader).unwrap(),
            Some(json!({ "jsonrpc": "2.0" }))
        );
        assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "id": 2 })));
    }

    #[test]
    fn typed_request_uses_lsp_types_method_constant() {
        assert_eq!(Initialize::METHOD, "initialize");
    }
}
