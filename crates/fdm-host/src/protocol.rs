//! The JSON messages exchanged with the browser extension.
//!
//! This is a private protocol between two halves of the same product, so it is
//! versioned rather than negotiated: the extension sends `protocol`, the host
//! compares it against [`PROTOCOL_VERSION`], and a mismatch produces a clear
//! "update FDM" error instead of a confusing parse failure. That matters because
//! the two halves update through completely separate channels — the extension
//! through the Chrome Web Store, the host through the Windows installer — so
//! they *will* be out of step on some machines.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bumped only on a breaking change to the shapes below.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Incoming {
    /// Liveness and version check. The welcome page uses this to show a real
    /// "connected" state rather than claiming success it has not verified.
    Ping {
        #[serde(default)]
        id: Option<u64>,
        #[serde(default)]
        protocol: Option<u32>,
    },

    /// Take over a download the extension cancelled in the browser.
    Download(Box<DownloadCommand>),

    /// Ask the engine to stop a running download. The partial file and its
    /// `.fdm` control file are left in place, so it resumes rather than
    /// restarts.
    Cancel { id: u64 },

    /// Everything currently running in this host process.
    Status {
        #[serde(default)]
        id: Option<u64>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadCommand {
    /// Correlation id chosen by the extension; echoed on every reply so the
    /// popup can match progress to a row.
    pub id: u64,
    pub url: String,

    /// Cookie, Referer and User-Agent, captured by the extension.
    ///
    /// Not optional in practice. The engine re-issues the request as a fresh
    /// anonymous client, so without these a download behind a login silently
    /// fetches the login page and saves *that* — a file of plausible size
    /// containing the wrong bytes.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,

    /// Filename Chrome had already resolved, when it had one. Left empty
    /// otherwise so the engine derives it from `Content-Disposition`, which it
    /// parses more carefully (RFC 5987, NTFS reserved names) than the extension
    /// could.
    #[serde(default)]
    pub filename: Option<String>,

    /// Total size, when the browser knew it. Advisory only, never used to size
    /// the file — the engine's own probe is authoritative. It is carried so a
    /// disagreement between the two can be logged, which is the fastest way to
    /// recognise a URL whose one-time token expired between the click and the
    /// handoff.
    #[serde(default)]
    pub total_bytes: Option<u64>,

    /// Override the download root. Absent means "use the configured root".
    #[serde(default)]
    pub target_dir: Option<String>,

    /// Separate audio stream URL for YouTube adaptive streams.
    /// When present, the backend downloads video and audio separately and
    /// merges with ffmpeg, completely bypassing yt-dlp.
    #[serde(default)]
    pub audio_url: Option<String>,

    #[serde(default)]
    pub protocol: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Outgoing {
    /// Reply to `ping`. Deliberately carries the paths and versions a support
    /// conversation needs, so "is it set up correctly?" is answerable from the
    /// welcome page alone.
    #[serde(rename_all = "camelCase")]
    Pong {
        id: Option<u64>,
        protocol: u32,
        version: &'static str,
        host_path: String,
        download_root: String,
        max_connections: u32,
        categories: Vec<&'static str>,
    },

    /// The host has taken ownership of a download. The extension may now stop
    /// worrying about the cancelled browser download.
    #[serde(rename_all = "camelCase")]
    Accepted { id: u64, url: String },

    #[serde(rename_all = "camelCase")]
    Progress {
        id: u64,
        downloaded: u64,
        total: Option<u64>,
        speed_bps: f64,
        eta_seconds: Option<u64>,
        segments: u32,
        active_connections: u32,
    },

    #[serde(rename_all = "camelCase")]
    Completed {
        id: u64,
        path: String,
        bytes: u64,
        seconds: f64,
        category: String,
        segments: u32,
        resumed: bool,
        used_ranges: bool,
    },

    /// A download stopped without finishing. `resumable` is the field the UI
    /// cares about: it distinguishes "press resume" from "this will never work".
    #[serde(rename_all = "camelCase")]
    Failed {
        id: u64,
        message: String,
        resumable: bool,
    },

    #[serde(rename_all = "camelCase")]
    Cancelled { id: u64 },

    #[serde(rename_all = "camelCase")]
    Status { id: Option<u64>, active: Vec<u64> },

    /// A malformed or unsupported message. Carries the id when one could be
    /// recovered, so the extension can fail one row instead of everything.
    #[serde(rename_all = "camelCase")]
    Error {
        id: Option<u64>,
        message: String,
        /// Set when the two halves of FDM are on incompatible versions, which
        /// needs a different message to the user than a transient failure.
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        version_mismatch: bool,
    },
}

impl Outgoing {
    pub fn error(id: Option<u64>, message: impl Into<String>) -> Self {
        Self::Error {
            id,
            message: message.into(),
            version_mismatch: false,
        }
    }

    pub fn version_mismatch(id: Option<u64>, theirs: u32) -> Self {
        Self::Error {
            id,
            message: format!(
                "The FDM extension speaks protocol v{theirs} but this copy of FDM speaks \
                 v{PROTOCOL_VERSION}. Update whichever is older: the extension from the \
                 Chrome Web Store, or FDM itself from its installer."
            ),
            version_mismatch: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_download_command() {
        let raw = r#"{
            "type": "download",
            "id": 7,
            "url": "https://example.com/big.iso",
            "headers": { "Cookie": "session=abc", "Referer": "https://example.com/" },
            "totalBytes": 1024,
            "protocol": 1
        }"#;
        let Incoming::Download(cmd) = serde_json::from_str(raw).unwrap() else {
            panic!("expected a download command");
        };
        assert_eq!(cmd.id, 7);
        assert_eq!(cmd.total_bytes, Some(1024));
        assert_eq!(cmd.headers.get("Cookie").unwrap(), "session=abc");
        // Absent optional fields must not be a parse error; the extension only
        // sends what the browser actually gave it.
        assert!(cmd.filename.is_none());
    }

    #[test]
    fn ping_needs_no_fields_at_all() {
        // The welcome page's very first message, sent before it knows anything.
        assert!(matches!(
            serde_json::from_str::<Incoming>(r#"{"type":"ping"}"#).unwrap(),
            Incoming::Ping { id: None, protocol: None }
        ));
    }

    #[test]
    fn serialises_progress_in_the_camel_case_the_extension_reads() {
        let json = serde_json::to_string(&Outgoing::Progress {
            id: 1,
            downloaded: 10,
            total: Some(100),
            speed_bps: 1.5,
            eta_seconds: Some(60),
            segments: 8,
            active_connections: 8,
        })
        .unwrap();
        assert!(json.contains(r#""type":"progress""#));
        assert!(json.contains(r#""speedBps":1.5"#));
        assert!(json.contains(r#""activeConnections":8"#));
    }

    #[test]
    fn ordinary_errors_omit_the_mismatch_flag() {
        let json = serde_json::to_string(&Outgoing::error(Some(3), "nope")).unwrap();
        assert!(!json.contains("versionMismatch"));

        let json = serde_json::to_string(&Outgoing::version_mismatch(Some(3), 99)).unwrap();
        assert!(json.contains(r#""versionMismatch":true"#));
    }
}
