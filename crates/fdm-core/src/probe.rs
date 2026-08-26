//! Discovering what the server will actually let us do.
//!
//! The critical question is not "does the server advertise `Accept-Ranges`" but
//! "does it *honour* a `Range` request", and those differ often enough in the
//! wild that trusting the header gets files corrupted. So we ask for one byte
//! and look at the status code: a `206` is proof, anything else is not.
//!
//! Getting this wrong in the permissive direction is unrecoverable — writing a
//! full `200` body at a segment offset produces a file that is the right size
//! and silently wrong. We therefore treat every ambiguous answer as "no ranges"
//! and fall back to a single sequential stream.

use reqwest::header::{
    HeaderMap, ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, ETAG, LAST_MODIFIED, RANGE,
};
use reqwest::{Client, Response, StatusCode};
use url::Url;

use crate::error::{Error, Result};
use crate::naming;

#[derive(Debug, Clone)]
pub struct RemoteInfo {
    /// URL after redirects — all segment requests use this to avoid re-walking
    /// the redirect chain on every connection.
    pub final_url: Url,
    pub total_size: Option<u64>,
    pub supports_ranges: bool,
    /// `ETag` if the server gave one, otherwise `Last-Modified`. Sent back as
    /// `If-Range` so a file that changes mid-download is detected instead of
    /// being stitched together from two different versions.
    pub validator: Option<String>,
    pub validator_is_etag: bool,
    pub mime: Option<String>,
    pub filename: String,
}

impl RemoteInfo {
    /// Whether parallel segmentation is possible at all. Needs both range
    /// support and a known size — without a size there is nothing to divide.
    pub fn can_segment(&self) -> bool {
        self.supports_ranges && self.total_size.map(|t| t > 0).unwrap_or(false)
    }
}

/// Interrogate the resource. `extra` carries headers handed over by the browser
/// extension (cookies, referer, user-agent) — without them, protected downloads
/// return a login page rather than the file.
pub async fn probe(client: &Client, url: &Url, extra: &HeaderMap) -> Result<RemoteInfo> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let probe_fut = client
            .get(url.clone())
            .headers(extra.clone())
            .header(RANGE, "bytes=0-0")
            .header(ACCEPT_ENCODING, "identity")
            .send();

        let response = match tokio::time::timeout(std::time::Duration::from_secs(6), probe_fut).await {
            Ok(Ok(res)) => res,
            Ok(Err(_e)) if attempt < 3 => {
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt)).await;
                continue;
            }
            Ok(Err(e)) => return Err(Error::Http(e)),
            Err(_) => return probe_without_range(client, url, extra).await,
        };

        let status = response.status();

        if status == StatusCode::PARTIAL_CONTENT {
            return Ok(from_partial_response(url, &response));
        }

        if status.is_success() {
            // Server ignored the Range header. Size comes from Content-Length; no
            // parallelism and no resume.
            return Ok(from_full_response(url, &response, false));
        }

        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
            if attempt < 4 {
                drop(response);
                tokio::time::sleep(std::time::Duration::from_millis(800 * attempt)).await;
                continue;
            }
        }

        // Some servers reject ranged GETs outright but serve a plain request fine.
        if matches!(
            status,
            StatusCode::METHOD_NOT_ALLOWED
                | StatusCode::NOT_IMPLEMENTED
                | StatusCode::RANGE_NOT_SATISFIABLE
                | StatusCode::BAD_REQUEST
        ) {
            drop(response);
            return probe_without_range(client, url, extra).await;
        }

        return Err(Error::Status(status.as_u16()));
    }
}

/// Fallback path: HEAD first, then a plain GET if HEAD is unsupported.
async fn probe_without_range(client: &Client, url: &Url, extra: &HeaderMap) -> Result<RemoteInfo> {
    let head = client
        .head(url.clone())
        .headers(extra.clone())
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await;

    if let Ok(response) = head {
        if response.status().is_success() {
            // Here the advertised header is all we have to go on.
            let advertises = advertises_ranges(response.headers());
            return Ok(from_full_response(url, &response, advertises));
        }
    }

    let response = client
        .get(url.clone())
        .headers(extra.clone())
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::Status(status.as_u16()));
    }
    Ok(from_full_response(url, &response, false))
}

fn from_partial_response(original: &Url, response: &Response) -> RemoteInfo {
    let headers = response.headers();
    // With a 206 the total is in Content-Range, not Content-Length (which here
    // describes only the single byte we asked for).
    let total_size = headers
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_content_range_total);

    let (validator, validator_is_etag) = extract_validator(headers);

    RemoteInfo {
        final_url: response.url().clone(),
        total_size,
        supports_ranges: true,
        validator,
        validator_is_etag,
        mime: header_string(headers, &CONTENT_TYPE),
        filename: derive_filename(original, response),
    }
}

fn from_full_response(original: &Url, response: &Response, supports_ranges: bool) -> RemoteInfo {
    let headers = response.headers();
    let total_size = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());

    let (validator, validator_is_etag) = extract_validator(headers);

    RemoteInfo {
        final_url: response.url().clone(),
        total_size,
        // Only claim range support if the caller proved it or the server
        // advertised it *and* we know the size.
        supports_ranges: supports_ranges && total_size.is_some(),
        validator,
        validator_is_etag,
        mime: header_string(headers, &CONTENT_TYPE),
        filename: derive_filename(original, response),
    }
}

fn advertises_ranges(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "none"
        })
        .unwrap_or(false)
}

/// Prefer `ETag`: it is an exact identity check, whereas `Last-Modified` has
/// one-second granularity and can miss a rapid edit. RFC 7233 also forbids
/// sending both in `If-Range`.
fn extract_validator(headers: &HeaderMap) -> (Option<String>, bool) {
    if let Some(etag) = header_string(headers, &ETAG) {
        // Weak validators (W/"...") are not valid in If-Range.
        if !etag.trim_start().starts_with("W/") {
            return (Some(etag), true);
        }
    }
    (header_string(headers, &LAST_MODIFIED), false)
}

fn header_string(headers: &HeaderMap, name: &reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `bytes 0-0/146515` -> `Some(146515)`. `bytes 0-0/*` -> `None`.
pub fn parse_content_range_total(value: &str) -> Option<u64> {
    let total = value.rsplit('/').next()?.trim();
    if total == "*" {
        return None;
    }
    total.parse::<u64>().ok()
}

/// `bytes 1024-2047/146515` -> `Some(1024)`. Used to verify the server sent the
/// range we actually asked for.
pub fn parse_content_range_start(value: &str) -> Option<u64> {
    let spec = value.trim().strip_prefix("bytes")?.trim();
    let range = spec.split('/').next()?.trim();
    range.split('-').next()?.trim().parse::<u64>().ok()
}

fn derive_filename(original: &Url, response: &Response) -> String {
    response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(naming::filename_from_content_disposition)
        // Prefer the post-redirect URL: redirects to a CDN usually carry the
        // real filename while the original is an opaque /download endpoint.
        .unwrap_or_else(|| {
            let from_final = naming::filename_from_url(response.url());
            if from_final == "download" {
                naming::filename_from_url(original)
            } else {
                from_final
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_total_from_content_range() {
        assert_eq!(parse_content_range_total("bytes 0-0/146515"), Some(146515));
        assert_eq!(parse_content_range_total("bytes 0-1023/146515"), Some(146515));
    }

    #[test]
    fn unknown_total_is_none() {
        assert_eq!(parse_content_range_total("bytes 0-0/*"), None);
    }

    #[test]
    fn reads_start_offset_from_content_range() {
        assert_eq!(parse_content_range_start("bytes 1024-2047/146515"), Some(1024));
        assert_eq!(parse_content_range_start("bytes 0-0/146515"), Some(0));
    }

    #[test]
    fn accept_ranges_none_means_unsupported() {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT_RANGES, "none".parse().unwrap());
        assert!(!advertises_ranges(&h));

        h.insert(ACCEPT_RANGES, "bytes".parse().unwrap());
        assert!(advertises_ranges(&h));
    }

    #[test]
    fn missing_accept_ranges_means_unsupported() {
        assert!(!advertises_ranges(&HeaderMap::new()));
    }

    #[test]
    fn weak_etag_is_rejected_as_validator() {
        let mut h = HeaderMap::new();
        h.insert(ETAG, "W/\"abc\"".parse().unwrap());
        h.insert(LAST_MODIFIED, "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap());
        let (validator, is_etag) = extract_validator(&h);
        assert!(!is_etag, "weak ETag must not be used for If-Range");
        assert_eq!(validator.as_deref(), Some("Wed, 21 Oct 2015 07:28:00 GMT"));
    }

    #[test]
    fn strong_etag_wins_over_last_modified() {
        let mut h = HeaderMap::new();
        h.insert(ETAG, "\"e4a2b-2f\"".parse().unwrap());
        h.insert(LAST_MODIFIED, "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap());
        let (validator, is_etag) = extract_validator(&h);
        assert!(is_etag);
        assert_eq!(validator.as_deref(), Some("\"e4a2b-2f\""));
    }

    #[test]
    fn cannot_segment_without_a_size() {
        let info = RemoteInfo {
            final_url: Url::parse("https://example.com/f").unwrap(),
            total_size: None,
            supports_ranges: true,
            validator: None,
            validator_is_etag: false,
            mime: None,
            filename: "f".into(),
        };
        assert!(!info.can_segment());
    }
}
