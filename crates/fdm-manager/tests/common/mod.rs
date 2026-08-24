//! A throttled HTTP server, just enough of one to test a download manager
//! against.
//!
//! The manager's interesting behaviour is all about *timing* — pause has to land
//! mid-transfer, a queue has to be observed holding a download back — and a real
//! server on the internet gives no control over either. So these tests serve
//! their own bytes, slowly and on purpose.
//!
//! It speaks the small part of HTTP the engine's probe actually requires: a
//! `Content-Length`, `Accept-Ranges: bytes`, a strong `ETag` to validate a resume
//! against, and `206` with a `Content-Range` for a ranged GET.

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct TestServer {
    pub addr: SocketAddr,
    /// GET requests served, ranged or not. Lets a test assert that a resume
    /// really did re-request rather than starting over.
    pub gets: Arc<AtomicUsize>,
    _task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    /// `total` bytes of deterministic filler, handed out in `chunk` pieces with
    /// `delay` between them so a transfer takes a predictable while.
    pub async fn start(total: usize, chunk: usize, delay: Duration) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let gets = Arc::new(AtomicUsize::new(0));

        let task = {
            let gets = gets.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = listener.accept().await else {
                        return;
                    };
                    let gets = gets.clone();
                    tokio::spawn(async move {
                        // A connection that fails is a client that went away —
                        // which is exactly what cancelling a download looks like.
                        let _ = serve(stream, total, chunk, delay, gets).await;
                    });
                }
            })
        };

        Ok(Self { addr, gets, _task: task })
    }

    pub fn url(&self, name: &str) -> String {
        format!("http://{}/{}", self.addr, name)
    }

    pub fn get_count(&self) -> usize {
        self.gets.load(Ordering::Relaxed)
    }
}

/// Byte `i` of the body. A position-dependent pattern, so a file assembled from
/// segments in the wrong order fails the check instead of passing by luck.
pub fn byte_at(i: usize) -> u8 {
    (i % 251) as u8
}

pub fn expected_body(total: usize) -> Vec<u8> {
    (0..total).map(byte_at).collect()
}

const ETAG: &str = "\"fdm-test-etag\"";

async fn serve(
    mut stream: TcpStream,
    total: usize,
    chunk: usize,
    delay: Duration,
    gets: Arc<AtomicUsize>,
) -> io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];

    // Read until the end of the request headers. No body handling: the engine
    // only ever sends HEAD and GET.
    loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Ok(());
        }
    }

    let text = String::from_utf8_lossy(&buf);
    let is_head = text.starts_with("HEAD ");
    let range = parse_range(&text, total);

    if is_head {
        let head = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Length: {total}\r\n\
             Content-Type: application/octet-stream\r\n\
             Accept-Ranges: bytes\r\n\
             ETag: {ETAG}\r\n\
             Connection: close\r\n\r\n"
        );
        stream.write_all(head.as_bytes()).await?;
        return stream.flush().await;
    }

    gets.fetch_add(1, Ordering::Relaxed);

    let (start, end) = range.unwrap_or((0, total.saturating_sub(1)));
    let len = end.saturating_sub(start) + 1;

    let head = if range.is_some() {
        format!(
            "HTTP/1.1 206 Partial Content\r\n\
             Content-Length: {len}\r\n\
             Content-Range: bytes {start}-{end}/{total}\r\n\
             Content-Type: application/octet-stream\r\n\
             Accept-Ranges: bytes\r\n\
             ETag: {ETAG}\r\n\
             Connection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Length: {len}\r\n\
             Content-Type: application/octet-stream\r\n\
             Accept-Ranges: bytes\r\n\
             ETag: {ETAG}\r\n\
             Connection: close\r\n\r\n"
        )
    };
    stream.write_all(head.as_bytes()).await?;

    let mut sent = 0usize;
    while sent < len {
        let n = chunk.min(len - sent);
        let body: Vec<u8> = (start + sent..start + sent + n).map(byte_at).collect();
        // A write error here is the client hanging up mid-transfer, which is what
        // pause and cancel do. Stop quietly.
        if stream.write_all(&body).await.is_err() {
            return Ok(());
        }
        sent += n;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }

    stream.flush().await
}

/// `Range: bytes=start-end`, clamped to the body. Open-ended forms included,
/// because the engine's sequential fallback sends `bytes=N-`.
fn parse_range(request: &str, total: usize) -> Option<(usize, usize)> {
    let line = request
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("range:"))?;
    let spec = line.split_once('=')?.1.trim();
    let (a, b) = spec.split_once('-')?;

    let start: usize = a.trim().parse().ok()?;
    let end = match b.trim() {
        "" => total.saturating_sub(1),
        v => v.parse::<usize>().ok()?.min(total.saturating_sub(1)),
    };
    if start > end {
        return None;
    }
    Some((start, end))
}
