//! LSP base protocol framing: `Content-Length: N\r\n\r\n<payload>`.
//!
//! Pure I/O over [`std::io::BufRead`] / [`std::io::Write`]; takes no
//! ownership of a transport so tests round-trip through `Vec<u8>` cursors.

use std::io::{self, BufRead, Write};

use thiserror::Error;

const CONTENT_LENGTH: &str = "content-length";

/// Read one framed LSP message from `reader` and return its payload bytes.
/// Returns [`CodecError::Eof`] when the stream cleanly ends between messages.
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, CodecError> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line).map_err(CodecError::Io)?;
        if bytes_read == 0 {
            return if content_length.is_none() {
                Err(CodecError::Eof)
            } else {
                Err(CodecError::TruncatedHeaders)
            };
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let Some((name, value)) = trimmed.split_once(':') else {
            return Err(CodecError::MalformedHeader(trimmed.to_string()));
        };
        if name.trim().eq_ignore_ascii_case(CONTENT_LENGTH) {
            let parsed = value
                .trim()
                .parse::<usize>()
                .map_err(|_| CodecError::MalformedHeader(trimmed.to_string()))?;
            content_length = Some(parsed);
        }
        // Other headers (e.g. Content-Type) are accepted but ignored.
    }

    let length = content_length.ok_or(CodecError::MissingContentLength)?;
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload).map_err(CodecError::Io)?;
    Ok(payload)
}

/// Write `payload` to `writer` with the required `Content-Length` header.
pub fn write_message<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), CodecError> {
    write!(writer, "Content-Length: {}\r\n\r\n", payload.len()).map_err(CodecError::Io)?;
    writer.write_all(payload).map_err(CodecError::Io)?;
    writer.flush().map_err(CodecError::Io)
}

/// LSP codec error.
#[derive(Debug, Error)]
pub enum CodecError {
    #[error("transport I/O failed: {0}")]
    Io(#[source] io::Error),
    #[error("end of stream before any message was read")]
    Eof,
    #[error("end of stream while reading message headers")]
    TruncatedHeaders,
    #[error("malformed message header: {0:?}")]
    MalformedHeader(String),
    #[error("message is missing the Content-Length header")]
    MissingContentLength,
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;

    #[test]
    fn write_then_read_round_trips_the_payload() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let mut buffer = Vec::new();
        write_message(&mut buffer, payload).unwrap();

        let mut reader = BufReader::new(Cursor::new(buffer));
        let read = read_message(&mut reader).unwrap();
        assert_eq!(read, payload);
    }

    #[test]
    fn header_is_case_insensitive() {
        let mut reader = BufReader::new(Cursor::new(b"content-length: 3\r\n\r\nabc".to_vec()));
        assert_eq!(read_message(&mut reader).unwrap(), b"abc");
    }

    #[test]
    fn ignores_other_headers_before_blank_line() {
        let mut reader = BufReader::new(Cursor::new(
            b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 2\r\n\r\nhi".to_vec(),
        ));
        assert_eq!(read_message(&mut reader).unwrap(), b"hi");
    }

    #[test]
    fn malformed_header_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(b"not-a-header\r\n\r\n".to_vec()));
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, CodecError::MalformedHeader(_)), "{err:?}");
    }

    #[test]
    fn missing_content_length_is_an_error() {
        let mut reader = BufReader::new(Cursor::new(b"Content-Type: x\r\n\r\nbody".to_vec()));
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, CodecError::MissingContentLength), "{err:?}");
    }

    #[test]
    fn empty_stream_reports_eof() {
        let mut reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, CodecError::Eof), "{err:?}");
    }

    #[test]
    fn truncated_headers_are_reported() {
        let mut reader = BufReader::new(Cursor::new(b"Content-Length: 5\r\n".to_vec()));
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, CodecError::TruncatedHeaders), "{err:?}");
    }

    #[test]
    fn multiple_messages_round_trip_in_sequence() {
        let mut buffer = Vec::new();
        write_message(&mut buffer, b"first").unwrap();
        write_message(&mut buffer, b"second").unwrap();
        write_message(&mut buffer, b"third").unwrap();

        let mut reader = BufReader::new(Cursor::new(buffer));
        assert_eq!(read_message(&mut reader).unwrap(), b"first");
        assert_eq!(read_message(&mut reader).unwrap(), b"second");
        assert_eq!(read_message(&mut reader).unwrap(), b"third");
        assert!(matches!(
            read_message(&mut reader).unwrap_err(),
            CodecError::Eof
        ));
    }
}
