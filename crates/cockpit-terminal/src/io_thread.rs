//! Dedicated terminal I/O threads.
//!
//! PTY reads are blocking, so the app receives output through channels rather
//! than reading on the UI thread.

use std::{
    io::{self, Read},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

/// Events emitted by the terminal reader thread.
#[derive(Debug, PartialEq, Eq)]
pub enum TerminalIoEvent {
    Output(Vec<u8>),
    Eof,
    ReadError(String),
}

/// Handle for a terminal reader thread.
pub struct ReaderThread {
    receiver: Receiver<TerminalIoEvent>,
    join: JoinHandle<()>,
}

impl ReaderThread {
    /// Receive the next event from the reader thread.
    pub fn recv(&self) -> Result<TerminalIoEvent, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Join the reader thread.
    pub fn join(self) -> thread::Result<()> {
        self.join.join()
    }
}

/// Spawn a blocking reader thread.
pub fn spawn_reader_thread(reader: impl Read + Send + 'static) -> ReaderThread {
    let (sender, receiver) = mpsc::channel();
    let join = thread::spawn(move || read_loop(reader, sender));
    ReaderThread { receiver, join }
}

fn read_loop(mut reader: impl Read, sender: Sender<TerminalIoEvent>) {
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = sender.send(TerminalIoEvent::Eof);
                break;
            }
            Ok(n) => {
                if sender
                    .send(TerminalIoEvent::Output(buffer[..n].to_vec()))
                    .is_err()
                {
                    break;
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => {
                let _ = sender.send(TerminalIoEvent::ReadError(err.to_string()));
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("reader failed"))
        }
    }

    #[test]
    fn reader_thread_emits_output_then_eof() {
        let thread = spawn_reader_thread(Cursor::new(b"hello".to_vec()));

        assert_eq!(
            thread.recv().unwrap(),
            TerminalIoEvent::Output(b"hello".to_vec())
        );
        assert_eq!(thread.recv().unwrap(), TerminalIoEvent::Eof);
        thread.join().unwrap();
    }

    #[test]
    fn reader_thread_emits_read_errors() {
        let thread = spawn_reader_thread(FailingReader);

        assert_eq!(
            thread.recv().unwrap(),
            TerminalIoEvent::ReadError("reader failed".to_string())
        );
        thread.join().unwrap();
    }
}
