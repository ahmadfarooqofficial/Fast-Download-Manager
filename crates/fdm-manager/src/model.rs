//! The shape of a row in the download list.
//!
//! Everything here is `serde`-serializable because the same structs travel three
//! ways: persisted to `downloads.json`, pushed to the desktop UI over IPC, and
//! returned from [`crate::Manager::list`]. One shape for all three means the UI
//! cannot drift from what was saved.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fdm_core::Category;
use serde::{Deserialize, Serialize};
use url::Url;

pub type DownloadId = u64;

/// Where a download is in its life.
///
/// Flat and stringly-typed on purpose: the UI switches on it to pick a colour
/// and a set of enabled buttons, and a tagged enum with payloads would make that
/// a destructuring exercise in JavaScript. The payload that would have lived on
/// `Failed` is [`DownloadEntry::error`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Accepted, waiting for a concurrency slot.
    Queued,
    /// Probing. The destination is not known yet, so the row has no filename.
    Connecting,
    /// Bytes are moving.
    Downloading,
    /// Stopped by the user, scratch files intact, resumable.
    Paused,
    Completed,
    /// Stopped by an error. Usually resumable — see [`DownloadEntry::resumable`].
    Failed,
    /// Stopped by the user, scratch files deleted. Resuming starts over.
    Cancelled,
}

impl Status {
    /// True while the manager owns a running task for this download.
    pub fn is_active(self) -> bool {
        matches!(self, Status::Queued | Status::Connecting | Status::Downloading)
    }

    /// True when there is nothing left to do and no task is running.
    pub fn is_finished(self) -> bool {
        matches!(self, Status::Completed | Status::Failed | Status::Cancelled)
    }
}

/// One download, as the UI sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEntry {
    pub id: DownloadId,
    pub url: String,
    /// Best name known so far. Taken from the request if it supplied one, else
    /// from the URL, then replaced with the engine's real choice once the probe
    /// has read `Content-Disposition`.
    pub filename: String,
    /// Final destination. `None` until the engine resolves it.
    pub path: Option<PathBuf>,
    pub status: Status,
    /// `None` when the server did not send a length. The UI must then draw an
    /// indeterminate bar rather than dividing by zero.
    pub total: Option<u64>,
    pub downloaded: u64,
    pub speed_bps: f64,
    pub eta_secs: Option<u64>,
    pub segments: u32,
    pub active_connections: u32,
    pub category: Option<Category>,
    /// Human-readable reason this failed. Set only alongside [`Status::Failed`].
    pub error: Option<String>,
    /// True when stopping and starting again would continue rather than restart.
    /// Drives whether the UI offers "Resume" or "Restart".
    pub resumable: bool,
    pub added_at: u64,
    pub finished_at: Option<u64>,
}

impl DownloadEntry {
    pub(crate) fn new(id: DownloadId, url: &str, filename: String) -> Self {
        Self {
            id,
            url: url.to_string(),
            filename,
            path: None,
            status: Status::Queued,
            total: None,
            downloaded: 0,
            speed_bps: 0.0,
            eta_secs: None,
            segments: 0,
            active_connections: 0,
            category: None,
            error: None,
            resumable: false,
            added_at: now_secs(),
            finished_at: None,
        }
    }

    /// Completed fraction, or `None` when the total is unknown.
    pub fn fraction(&self) -> Option<f64> {
        self.total
            .filter(|t| *t > 0)
            .map(|t| (self.downloaded as f64 / t as f64).min(1.0))
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the UI or the browser bridge hands to [`crate::Manager::add`].
#[derive(Debug, Clone)]
pub struct NewDownload {
    pub url: Url,
    /// Cookies, referer and user-agent from the browser. Held in memory for the
    /// life of the process and deliberately never persisted — see the crate docs.
    pub headers: fdm_core::HeaderMap,
    pub filename: Option<String>,
    /// Set only when the user picked a folder. Leaving it `None` is what makes
    /// the engine sort into `Downloads\FDM\<Category>\`.
    pub target_dir: Option<PathBuf>,
    /// Separate audio stream URL for YouTube adaptive formats. When set, the
    /// manager downloads video+audio in parallel and merges with ffmpeg.
    pub audio_url: Option<String>,
}

impl NewDownload {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            headers: fdm_core::HeaderMap::new(),
            filename: None,
            target_dir: None,
            audio_url: None,
        }
    }

    pub fn with_headers(mut self, headers: fdm_core::HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    pub fn with_filename(mut self, name: impl Into<String>) -> Self {
        self.filename = Some(name.into());
        self
    }

    pub fn with_target_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.target_dir = Some(dir.into());
        self
    }

    pub fn with_audio_url(mut self, url: impl Into<String>) -> Self {
        self.audio_url = Some(url.into());
        self
    }
}

/// Pushed to every subscriber whenever the list changes.
///
/// `Changed` carries the whole entry rather than a delta: a subscriber that
/// missed a message still ends up correct, and the UI's update path is a single
/// "replace the row with this id" for every kind of change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Added(DownloadEntry),
    Changed(DownloadEntry),
    Removed(DownloadId),
}
