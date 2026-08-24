//! Chrome's native messaging wire format.
//!
//! Every message is a 32-bit length prefix in **native byte order** followed by
//! that many bytes of UTF-8 JSON. Three details in that sentence are the ones
//! people get wrong:
//!
//! * *Native* byte order, not network byte order. Chrome writes the length with
//!   a plain memory copy of a `uint32`, so on x86/ARM it is little-endian. Using
//!   `to_be_bytes` here produces a host that hangs forever on the first message.
//! * The length is a byte count, not a character count. Multi-byte UTF-8 in a
//!   filename or cookie makes those two differ.
//! * stdout carries the protocol and nothing else. A single stray `println!`
//!   corrupts the stream and Chrome closes the port with no diagnostic beyond
//!   "Error when communicating with the native messaging host". All logging in
//!   this crate goes to stderr, which Chrome captures into its own log.
//!
//! Windows note: the usual C advice is to `_setmode(_O_BINARY)` on the standard
//! handles so the CRT stops translating `\n` into `\r\n` — a translation that
//! would silently corrupt binary length prefixes. Rust does not need it. Its
//! `Stdin`/`Stdout` wrap the raw `HANDLE` from `GetStdHandle` and call
//! `ReadFile`/`WriteFile` directly, bypassing the CRT's text mode entirely.

use std::io::{self, Read, Write};

/// Refuse to allocate for a length prefix larger than this.
///
/// The cap is the point of the check: a corrupt or hostile prefix of
/// `0xFFFFFFFF` would otherwise have us reserve 4 GiB before reading a byte.
/// Chrome's own limit on extension-to-host messages is far below this.
pub const MAX_INCOMING: u32 = 64 * 1024 * 1024;

/// Chrome hard-caps a single host-to-extension message at 1 MiB and drops the
/// port when it is exceeded. Better to fail loudly here than to have the port
/// die with no explanation.
pub const MAX_OUTGOING: usize = 1024 * 1024;

/// Read one message. `Ok(None)` means Chrome closed the pipe, which is the
/// normal way a native messaging session ends and must not be treated as an
/// error.
pub fn read_message(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len) {
        Ok(()) => {}
        // A clean EOF on the length prefix is the port closing.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_ne_bytes(len);
    if len == 0 {
        return Ok(Some(Vec::new()));
    }
    if len > MAX_INCOMING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message length {len} exceeds the {MAX_INCOMING} byte limit"),
        ));
    }

    let mut body = vec![0u8; len as usize];
    // A truncated body *is* an error, unlike a truncated prefix: the sender
    // promised n bytes and delivered fewer, so the stream is out of sync and
    // cannot be resynchronised.
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Write one message and flush. Flushing matters: Chrome blocks waiting for a
/// reply, and a buffered response looks exactly like a hung host.
pub fn write_message(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_OUTGOING {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "response of {} bytes exceeds Chrome's {MAX_OUTGOING} byte limit",
                body.len()
            ),
        ));
    }

    let len = u32::try_from(body.len()).expect("checked against MAX_OUTGOING above");
    writer.write_all(&len.to_ne_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_message() {
        let mut buf = Vec::new();
        write_message(&mut buf, br#"{"type":"ping"}"#).unwrap();

        let mut cursor = io::Cursor::new(buf);
        let got = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(got, br#"{"type":"ping"}"#);
        // Second read hits EOF, which is a closed port and not a failure.
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn prefix_is_native_order_not_network_order() {
        let mut buf = Vec::new();
        write_message(&mut buf, b"hi").unwrap();
        assert_eq!(&buf[..4], &2u32.to_ne_bytes());
    }

    #[test]
    fn counts_bytes_not_characters() {
        // "é" is two bytes of UTF-8. A character count would write 1 here and
        // desynchronise the stream on the very next message.
        let mut buf = Vec::new();
        write_message(&mut buf, "é".as_bytes()).unwrap();
        assert_eq!(&buf[..4], &2u32.to_ne_bytes());
    }

    #[test]
    fn rejects_an_absurd_length_prefix() {
        let mut stream = u32::MAX.to_ne_bytes().to_vec();
        stream.extend_from_slice(b"nowhere near 4 GiB");
        let err = read_message(&mut io::Cursor::new(stream)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_truncated_body_is_an_error() {
        let mut stream = 16u32.to_ne_bytes().to_vec();
        stream.extend_from_slice(b"only 9 by");
        assert!(read_message(&mut io::Cursor::new(stream)).is_err());
    }

    #[test]
    fn refuses_to_exceed_chromes_response_cap() {
        let big = vec![b'x'; MAX_OUTGOING + 1];
        assert!(write_message(&mut Vec::new(), &big).is_err());
    }
}
