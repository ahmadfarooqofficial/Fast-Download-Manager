//! Length-prefixed framing over the pipe.
//!
//! Deliberately the same shape as Chrome's native messaging wire, which
//! `fdm-host` already speaks: a 32-bit length in **native** byte order, then that
//! many bytes of UTF-8 JSON. One framing idea in the product means one set of
//! off-by-one mistakes to have already made — see `fdm-host/src/framing.rs` for
//! the three that bite.
//!
//! A named pipe is a stream, not a datagram channel, even in message mode. A read
//! can return half a frame or two and a half frames, so the length prefix is not
//! decoration: it is the only thing that says where a message ends.

use std::io;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Refuse to allocate for a prefix larger than this.
///
/// The cap is the point: a corrupt prefix of `0xFFFFFFFF` would otherwise have us
/// reserve 4 GiB before reading a byte. Sized for the one message that can
/// legitimately be large — a `List` reply — at roughly 400 bytes a row, which
/// leaves room for far more downloads than a person will ever keep in the list.
pub const MAX_FRAME: u32 = 32 * 1024 * 1024;

/// Read one frame. `Ok(None)` is a clean disconnect, which is how every
/// well-behaved client ends a session and must not be logged as a failure.
pub async fn read_frame<R>(reader: &mut R) -> io::Result<Option<Vec<u8>>>
where
    R: AsyncRead + Unpin,
{
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        // EOF *on the prefix* is the peer hanging up between messages.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        // What a Windows named pipe reports when the other end closes its handle.
        // Indistinguishable from a clean disconnect at this layer, and treating
        // it as an error would fill the log with noise every time a client exits.
        Err(e) if is_disconnect(&e) => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_ne_bytes(len);
    if len == 0 {
        return Ok(Some(Vec::new()));
    }
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} exceeds the {MAX_FRAME} byte limit"),
        ));
    }

    let mut body = vec![0u8; len as usize];
    // A truncated *body* is a real error, unlike a truncated prefix: the sender
    // promised n bytes and delivered fewer, so the stream is out of sync and
    // there is no way to resynchronise it.
    reader.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Write one frame and flush.
///
/// Flushing matters on a pipe for the same reason it matters on stdio: the peer is
/// blocked in a read, and a buffered reply is indistinguishable from a hung
/// server.
pub async fn write_frame<W>(writer: &mut W, body: &[u8]) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let len = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {} bytes does not fit a 32-bit prefix", body.len()),
        )
    })?;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME} byte limit"),
        ));
    }

    // One `write_all` for prefix and body together. Two calls would let a
    // concurrent writer interleave between them and desynchronise the stream —
    // which is why every writer in this crate is a single task owning the write
    // half, and why this function does not take a shared handle.
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&len.to_ne_bytes());
    framed.extend_from_slice(body);
    writer.write_all(&framed).await?;
    writer.flush().await
}

/// Read one frame and parse it. `Ok(None)` is still a clean disconnect.
pub async fn read_json<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    match read_frame(reader).await? {
        None => Ok(None),
        Some(body) => serde_json::from_slice(&body)
            .map(Some)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

pub async fn write_json<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let body = serde_json::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_frame(writer, &body).await
}

/// Whether this error is the peer having gone away rather than something wrong.
///
/// `BrokenPipe` and `ConnectionReset` are the portable spellings.
/// `ERROR_BROKEN_PIPE` (109) and `ERROR_PIPE_NOT_CONNECTED` (233) are what a
/// Windows named pipe actually returns, and at least one of them lands in
/// `ErrorKind::Uncategorized` — the same trap as `fdm-host/src/lock.rs`, so the
/// raw code is checked too.
pub fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset | io::ErrorKind::UnexpectedEof
    ) || matches!(e.raw_os_error(), Some(109) | Some(233))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct Msg {
        text: String,
    }

    #[tokio::test]
    async fn round_trips_two_frames_from_one_buffer() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"first").await.unwrap();
        write_frame(&mut buf, b"second").await.unwrap();

        // A single stream carrying two messages is the normal case, and the
        // reason the prefix exists.
        let mut cursor = &buf[..];
        assert_eq!(read_frame(&mut cursor).await.unwrap().unwrap(), b"first");
        assert_eq!(read_frame(&mut cursor).await.unwrap().unwrap(), b"second");
        assert!(read_frame(&mut cursor).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_prefix_is_native_order_and_counts_bytes() {
        let mut buf = Vec::new();
        // "é" is two bytes of UTF-8; a character count would write 1 and
        // desynchronise the stream on the very next frame.
        write_frame(&mut buf, "é".as_bytes()).await.unwrap();
        assert_eq!(&buf[..4], &2u32.to_ne_bytes());
    }

    #[tokio::test]
    async fn json_survives_the_trip() {
        let mut buf = Vec::new();
        let msg = Msg {
            text: "café".into(),
        };
        write_json(&mut buf, &msg).await.unwrap();
        let back: Msg = read_json(&mut &buf[..]).await.unwrap().unwrap();
        assert_eq!(back, msg);
    }

    #[tokio::test]
    async fn rejects_an_absurd_length_prefix() {
        let mut stream = u32::MAX.to_ne_bytes().to_vec();
        stream.extend_from_slice(b"nowhere near 4 GiB");
        let err = read_frame(&mut &stream[..]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_truncated_body_is_an_error_but_a_truncated_prefix_is_not() {
        let mut stream = 16u32.to_ne_bytes().to_vec();
        stream.extend_from_slice(b"only 9 by");
        assert!(read_frame(&mut &stream[..]).await.is_err());

        // Two bytes of a four-byte prefix: the peer died between messages.
        assert!(read_frame(&mut &[0u8, 0][..]).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn malformed_json_is_reported_as_bad_data_not_as_a_disconnect() {
        // The distinction the server acts on: bad data gets an error reply and
        // the connection stays up; a disconnect ends the session.
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{ not json").await.unwrap();
        let err = read_json::<_, Msg>(&mut &buf[..]).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(!is_disconnect(&err));
    }

    #[test]
    fn a_closed_pipe_is_recognised_by_its_raw_code() {
        // 109 is ERROR_BROKEN_PIPE. Rust maps it to `BrokenPipe` on some paths
        // and leaves it uncategorised on others, so both routes must work.
        assert!(is_disconnect(&io::Error::from_raw_os_error(109)));
        assert!(is_disconnect(&io::Error::from_raw_os_error(233)));
        assert!(!is_disconnect(&io::Error::from(io::ErrorKind::InvalidData)));
    }
}
