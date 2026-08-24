//! Turning browser-supplied headers into headers the engine can safely send.
//!
//! Downloads arrive from the browser carrying whatever the page had: cookies, a
//! referer, a user-agent, and occasionally headers that would actively break a
//! segmented transfer. Sanitising them is an *engine* concern rather than a
//! browser-bridge concern, which is why this lives here: `Accept-Encoding` is
//! dangerous because of how byte ranges work, and every path that reaches
//! [`crate::Engine::download`] with third-party headers needs the same guard.
//! There are two such paths now — the native messaging host and the IPC pipe —
//! and one shared rule beats two copies that drift.

use crate::{HeaderMap, HeaderName, HeaderValue};

/// Headers the caller is not allowed to dictate.
///
/// `Accept-Encoding` is the dangerous one and the reason this list exists at all.
/// The engine controls it deliberately, because a gzipped response makes byte
/// ranges refer to *compressed* offsets and every segment boundary lands in the
/// wrong place — a corrupt file of exactly the right size. The rest are
/// connection-level headers belonging to whichever HTTP stack is actually holding
/// the socket; forwarding a stale `Content-Length` or `Range` would be worse than
/// useless.
pub const DENYLIST: &[&str] = &[
    "accept-encoding",
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-connection",
    "range",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// True when the engine reserves this header for itself.
pub fn is_denied(name: &str) -> bool {
    DENYLIST.contains(&name.to_ascii_lowercase().as_str())
}

/// Build a [`HeaderMap`] from arbitrary name/value pairs, dropping anything on
/// the denylist and anything HTTP itself would reject.
///
/// Invalid entries are skipped rather than failing the download: the input comes
/// from whatever a website happened to set in a cookie, and one weird byte in one
/// cookie should not be the reason a 4 GB download refuses to start.
pub fn sanitize<I, K, V>(pairs: I) -> HeaderMap
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let iter = pairs.into_iter();
    let mut map = HeaderMap::with_capacity(iter.size_hint().0);

    for (name, value) in iter {
        let name = name.as_ref();
        let lower = name.to_ascii_lowercase();
        if DENYLIST.contains(&lower.as_str()) {
            tracing::debug!(header = %name, "dropped: the engine controls this header");
            continue;
        }
        match (
            HeaderName::from_bytes(lower.as_bytes()),
            HeaderValue::from_str(value.as_ref()),
        ) {
            (Ok(n), Ok(v)) => {
                map.insert(n, v);
            }
            _ => tracing::debug!(header = %name, "dropped: not a valid HTTP header"),
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_headers_that_make_authenticated_downloads_work() {
        let map = sanitize([
            ("Cookie", "session=abc123"),
            ("Referer", "https://example.com/page"),
            ("User-Agent", "Mozilla/5.0"),
        ]);
        assert_eq!(map.get("cookie").unwrap(), "session=abc123");
        assert_eq!(map.get("referer").unwrap(), "https://example.com/page");
        assert_eq!(map.get("user-agent").unwrap(), "Mozilla/5.0");
    }

    #[test]
    fn drops_accept_encoding_whatever_case_it_arrives_in() {
        let map = sanitize([
            ("Accept-Encoding", "gzip, deflate, br"),
            ("accept-encoding", "gzip"),
            ("ACCEPT-ENCODING", "br"),
        ]);
        assert!(map.get("accept-encoding").is_none());
    }

    #[test]
    fn drops_connection_level_headers() {
        let map = sanitize([
            ("Host", "evil.example"),
            ("Content-Length", "0"),
            ("Range", "bytes=0-10"),
            ("Transfer-Encoding", "chunked"),
            ("Cookie", "keep=me"),
        ]);
        assert_eq!(map.len(), 1, "only Cookie should survive");
        assert!(map.get("cookie").is_some());
    }

    #[test]
    fn one_malformed_cookie_does_not_lose_the_others() {
        let map = sanitize([
            ("Cookie", "fine=yes"),
            ("X-Bad", "line\nbreak"),
            ("Referer", "https://example.com/"),
        ]);
        assert!(map.get("x-bad").is_none());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn is_denied_is_case_insensitive() {
        assert!(is_denied("Accept-Encoding"));
        assert!(is_denied("RANGE"));
        assert!(!is_denied("Cookie"));
    }
}
