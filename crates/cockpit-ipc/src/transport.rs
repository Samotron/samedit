//! Unix-domain-socket transport.
//!
//! The server (whichever process is the host) binds a socket and accepts
//! [`Connection`]s; clients connect and exchange [`Envelope`]s. The socket
//! lives in a user-only directory (`$XDG_RUNTIME_DIR`), so this is explicitly
//! *not* a remote protocol — there is no cross-user reach and no auth layer.
//!
//! Unix only. The Windows named-pipe transport (`\\.\pipe\cockpit`,
//! [`crate::WINDOWS_PIPE_NAME`]) is a follow-up; this module is `#[cfg(unix)]`
//! and the rest of the crate (wire types + codec) is platform-independent.

use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::codec::{read_message, write_message};
use crate::error::Result;
use crate::wire::{Encoding, Envelope};

/// The default socket path: `$XDG_RUNTIME_DIR/cockpit.sock`, falling back to
/// the system temp dir when the runtime dir is unset.
pub fn default_socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("cockpit.sock"),
        _ => std::env::temp_dir().join("cockpit.sock"),
    }
}

/// One end of an IPC conversation. Send and receive borrow `&self` — a
/// `UnixStream` is full-duplex, so a single connection can carry requests one
/// way and responses the other.
#[derive(Debug)]
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    /// Connect to a server listening at `path`.
    pub fn connect(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Connection {
            stream: UnixStream::connect(path)?,
        })
    }

    /// Wrap an already-accepted stream.
    pub fn from_stream(stream: UnixStream) -> Self {
        Connection { stream }
    }

    /// Send one envelope with the given encoding.
    pub fn send<T: Serialize>(&self, envelope: &Envelope<T>, encoding: Encoding) -> Result<()> {
        let mut w = &self.stream;
        write_message(&mut w, envelope, encoding)
    }

    /// Send one envelope using the production (CBOR) encoding.
    pub fn send_cbor<T: Serialize>(&self, envelope: &Envelope<T>) -> Result<()> {
        self.send(envelope, Encoding::Cbor)
    }

    /// Receive the next envelope, decoding whichever encoding the sender used.
    pub fn recv<T: DeserializeOwned>(&self) -> Result<Envelope<T>> {
        let mut r = &self.stream;
        read_message(&mut r)
    }

    /// Duplicate the connection handle (shares the underlying socket).
    pub fn try_clone(&self) -> Result<Self> {
        Ok(Connection {
            stream: self.stream.try_clone()?,
        })
    }
}

/// A bound server socket. Dropping it removes the socket file.
#[derive(Debug)]
pub struct IpcListener {
    listener: UnixListener,
    path: PathBuf,
}

impl IpcListener {
    /// Bind a server socket at `path`, removing any stale socket file left by a
    /// previous (crashed) instance first.
    pub fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        // A leftover socket file would make `bind` fail with EADDRINUSE even
        // though no one is listening. Best-effort cleanup; ignore "not found".
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        Ok(IpcListener { listener, path })
    }

    /// Accept the next client connection (blocking).
    pub fn accept(&self) -> Result<Connection> {
        let (stream, _addr) = self.listener.accept()?;
        Ok(Connection::from_stream(stream))
    }

    /// The path this listener is bound to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IpcListener {
    fn drop(&mut self) {
        // Leave the runtime dir clean for the next host.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ServiceId;
    use serde::Deserialize;
    use std::thread;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum Msg {
        Ping,
        Pong(u32),
    }

    fn sock_path() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        // Keep the dir alive by leaking it — the test process is short-lived.
        let path = dir.path().join("cockpit.sock");
        std::mem::forget(dir);
        path
    }

    #[test]
    fn client_server_round_trip_in_order() {
        let path = sock_path();
        let listener = IpcListener::bind(&path).unwrap();

        let server = thread::spawn(move || {
            let conn = listener.accept().unwrap();
            for _ in 0..3 {
                let env: Envelope<Msg> = conn.recv().unwrap();
                assert_eq!(env.service, ServiceId::Jot);
                if let Msg::Pong(n) = env.payload {
                    conn.send_cbor(&Envelope::new(ServiceId::Jot, Msg::Pong(n + 1)))
                        .unwrap();
                }
            }
        });

        let client = Connection::connect(&path).unwrap();
        for n in 0..3 {
            client
                .send_cbor(&Envelope::new(ServiceId::Jot, Msg::Pong(n)))
                .unwrap();
            let reply: Envelope<Msg> = client.recv().unwrap();
            assert_eq!(reply.payload, Msg::Pong(n + 1));
        }
        server.join().unwrap();
    }

    #[test]
    fn server_accepts_reconnect() {
        let path = sock_path();
        let listener = IpcListener::bind(&path).unwrap();

        let server = thread::spawn(move || {
            // Two separate client sessions, one after another.
            for _ in 0..2 {
                let conn = listener.accept().unwrap();
                let env: Envelope<Msg> = conn.recv().unwrap();
                conn.send_cbor(&Envelope::new(env.service, Msg::Ping))
                    .unwrap();
            }
        });

        for _ in 0..2 {
            let client = Connection::connect(&path).unwrap();
            client
                .send_cbor(&Envelope::new(ServiceId::Cockpit, Msg::Ping))
                .unwrap();
            let reply: Envelope<Msg> = client.recv().unwrap();
            assert_eq!(reply.payload, Msg::Ping);
            drop(client);
        }
        server.join().unwrap();
    }

    #[test]
    fn json_and_cbor_interoperate_on_one_socket() {
        let path = sock_path();
        let listener = IpcListener::bind(&path).unwrap();

        let server = thread::spawn(move || {
            let conn = listener.accept().unwrap();
            // First message arrives as JSON, second as CBOR; both decode.
            let a: Envelope<Msg> = conn.recv().unwrap();
            let b: Envelope<Msg> = conn.recv().unwrap();
            (a.payload, b.payload)
        });

        let client = Connection::connect(&path).unwrap();
        client
            .send(&Envelope::new(ServiceId::Jot, Msg::Pong(1)), Encoding::Json)
            .unwrap();
        client
            .send(&Envelope::new(ServiceId::Jot, Msg::Pong(2)), Encoding::Cbor)
            .unwrap();

        let (a, b) = server.join().unwrap();
        assert_eq!(a, Msg::Pong(1));
        assert_eq!(b, Msg::Pong(2));
    }

    #[test]
    fn bind_replaces_stale_socket_file() {
        let path = sock_path();
        let first = IpcListener::bind(&path).unwrap();
        drop(first); // removes the file
        // A stray file in the way must not block a fresh bind.
        std::fs::write(&path, b"stale").unwrap();
        let _second = IpcListener::bind(&path).unwrap();
    }
}
