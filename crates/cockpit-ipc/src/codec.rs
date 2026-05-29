//! Length-prefixed framing.
//!
//! Each frame is: a 4-byte big-endian length, then that many body bytes. The
//! body is a 1-byte encoding tag ([`Encoding`]) followed by the encoded
//! [`Envelope`]. The tag lets one socket accept CBOR (production) or JSON
//! (debug) frames interchangeably.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{IpcError, Result};
use crate::wire::{Encoding, Envelope};

/// Largest body we will read or write (16 MiB). Guards against a corrupt or
/// hostile length prefix turning into a giant allocation.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Encode `envelope` with `encoding` and write one framed message to `w`.
pub fn write_message<W: Write, T: Serialize>(
    w: &mut W,
    envelope: &Envelope<T>,
    encoding: Encoding,
) -> Result<()> {
    let payload = match encoding {
        Encoding::Cbor => {
            let mut buf = Vec::new();
            ciborium::into_writer(envelope, &mut buf)
                .map_err(|e| IpcError::CborEncode(e.to_string()))?;
            buf
        }
        Encoding::Json => serde_json::to_vec(envelope)?,
    };

    let body_len = payload.len() + 1; // +1 for the encoding tag
    if body_len > MAX_FRAME {
        return Err(IpcError::FrameTooLarge {
            len: body_len,
            max: MAX_FRAME,
        });
    }

    w.write_all(&(body_len as u32).to_be_bytes())?;
    w.write_all(&[encoding.tag()])?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

/// Read one framed message from `r` and decode it.
pub fn read_message<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<Envelope<T>> {
    let mut len_buf = [0u8; 4];
    read_exact(r, &mut len_buf)?;
    let body_len = u32::from_be_bytes(len_buf) as usize;

    if body_len == 0 {
        // A body must carry at least the encoding tag.
        return Err(IpcError::UnexpectedEof);
    }
    if body_len > MAX_FRAME {
        return Err(IpcError::FrameTooLarge {
            len: body_len,
            max: MAX_FRAME,
        });
    }

    let mut body = vec![0u8; body_len];
    read_exact(r, &mut body)?;

    let tag = body[0];
    let encoding = Encoding::from_tag(tag).ok_or(IpcError::UnknownEncoding(tag))?;
    let payload = &body[1..];

    let envelope = match encoding {
        Encoding::Cbor => {
            ciborium::from_reader(payload).map_err(|e| IpcError::CborDecode(e.to_string()))?
        }
        Encoding::Json => serde_json::from_slice(payload)?,
    };
    Ok(envelope)
}

/// `read_exact`, but a clean EOF before any byte maps to [`IpcError::UnexpectedEof`].
fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    match r.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Err(IpcError::UnexpectedEof),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::ServiceId;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum Msg {
        Ping,
        Echo(String),
    }

    fn round_trip(encoding: Encoding) {
        let env = Envelope::new(ServiceId::Jot, Msg::Echo("hi".into()));
        let mut buf = Vec::new();
        write_message(&mut buf, &env, encoding).unwrap();
        let got: Envelope<Msg> = read_message(&mut buf.as_slice()).unwrap();
        assert_eq!(got, env);
    }

    #[test]
    fn cbor_round_trip() {
        round_trip(Encoding::Cbor);
    }

    #[test]
    fn json_round_trip() {
        round_trip(Encoding::Json);
    }

    #[test]
    fn multiple_messages_preserve_order() {
        let mut buf = Vec::new();
        for i in 0..5 {
            let env = Envelope::new(ServiceId::Cockpit, Msg::Echo(i.to_string()));
            write_message(&mut buf, &env, Encoding::Cbor).unwrap();
        }
        let mut r = buf.as_slice();
        for i in 0..5 {
            let env: Envelope<Msg> = read_message(&mut r).unwrap();
            assert_eq!(env.payload, Msg::Echo(i.to_string()));
        }
        // Nothing left — next read hits clean EOF.
        assert!(matches!(
            read_message::<_, Msg>(&mut r),
            Err(IpcError::UnexpectedEof)
        ));
    }

    #[test]
    fn unknown_encoding_tag_is_rejected() {
        // len = 2 (tag + 1 byte), tag = 9 (invalid).
        let frame = [0, 0, 0, 2, 9, 42];
        let err = read_message::<_, Msg>(&mut frame.as_slice()).unwrap_err();
        assert!(matches!(err, IpcError::UnknownEncoding(9)));
    }

    #[test]
    fn truncated_body_is_unexpected_eof() {
        // Claims 10 body bytes but only supplies 3.
        let frame = [0, 0, 0, 10, 0, 1, 2];
        let err = read_message::<_, Msg>(&mut frame.as_slice()).unwrap_err();
        assert!(matches!(err, IpcError::UnexpectedEof));
    }

    #[test]
    fn oversize_length_is_rejected_without_allocating() {
        let frame = [0xff, 0xff, 0xff, 0xff]; // ~4 GiB body claim
        let err = read_message::<_, Msg>(&mut frame.as_slice()).unwrap_err();
        assert!(matches!(err, IpcError::FrameTooLarge { .. }));
    }
}
