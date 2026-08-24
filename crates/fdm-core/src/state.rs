//! Resume state, persisted next to the partial file as `<target>.part.fdm`.
//!
//! A download manager that loses a 9 GB transfer to a power cut has failed at
//! its one job, so segment cursors are checkpointed to disk on a timer. The
//! control file is only ever replaced atomically — a half-written control file
//! would be worse than none at all, because it would make us resume from bogus
//! offsets and silently corrupt the output.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::plan::SegmentSnapshot;
use crate::probe::RemoteInfo;

/// Bumped whenever the on-disk shape changes. An unrecognised version is
/// discarded rather than guessed at.
pub const CONTROL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadState {
    pub version: u32,
    /// URL the user originally asked for.
    pub url: String,
    /// URL after redirects, which is what segments actually fetch.
    pub final_url: String,
    pub total_size: Option<u64>,
    pub validator: Option<String>,
    pub validator_is_etag: bool,
    pub filename: String,
    pub target: PathBuf,
    pub segments: Vec<SegmentSnapshot>,
}

impl DownloadState {
    pub fn new(url: &str, info: &RemoteInfo, target: &Path, segments: Vec<SegmentSnapshot>) -> Self {
        Self {
            version: CONTROL_VERSION,
            url: url.to_string(),
            final_url: info.final_url.to_string(),
            total_size: info.total_size,
            validator: info.validator.clone(),
            validator_is_etag: info.validator_is_etag,
            filename: info.filename.clone(),
            target: target.to_path_buf(),
            segments,
        }
    }

    /// Read existing state. Returns `None` for a missing, unreadable, corrupt,
    /// or future-versioned file — in every one of those cases starting over is
    /// the correct and safe answer.
    pub fn load(path: &Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        let state: DownloadState = serde_json::from_slice(&bytes).ok()?;
        if state.version != CONTROL_VERSION {
            tracing::warn!(
                found = state.version,
                expected = CONTROL_VERSION,
                "discarding control file with unsupported version"
            );
            return None;
        }
        if state.segments.is_empty() {
            return None;
        }
        Some(state)
    }

    /// Write via a temporary file and rename, so an interrupted save leaves the
    /// previous good state in place. `std::fs::rename` replaces the destination
    /// on Windows as well as unix.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = {
            let mut s = path.as_os_str().to_owned();
            s.push(".tmp");
            PathBuf::from(s)
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Is this state still valid for what the server is serving right now?
    ///
    /// A changed size or validator means the remote file was replaced, so the
    /// bytes already on disk belong to a different file and must be thrown away.
    pub fn is_resumable_for(&self, url: &str, info: &RemoteInfo) -> bool {
        if self.url != url {
            return false;
        }
        if self.total_size != info.total_size {
            tracing::info!("remote size changed; cannot resume");
            return false;
        }
        // A server that stopped sending a validator gives us no way to prove the
        // file is unchanged. Resuming would be a guess.
        match (&self.validator, &info.validator) {
            (Some(old), Some(new)) => {
                if old != new {
                    tracing::info!("validator changed; cannot resume");
                    return false;
                }
            }
            (Some(_), None) | (None, Some(_)) => return false,
            (None, None) => {}
        }
        info.supports_ranges
    }

    pub fn bytes_done(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| {
                let len = s.end.saturating_sub(s.start).saturating_add(1);
                s.done.min(len)
            })
            .sum()
    }

    pub fn delete(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn info(size: Option<u64>, validator: Option<&str>, ranges: bool) -> RemoteInfo {
        RemoteInfo {
            final_url: Url::parse("https://example.com/f.bin").unwrap(),
            total_size: size,
            supports_ranges: ranges,
            validator: validator.map(str::to_string),
            validator_is_etag: true,
            mime: None,
            filename: "f.bin".into(),
        }
    }

    fn state(size: Option<u64>, validator: Option<&str>) -> DownloadState {
        DownloadState {
            version: CONTROL_VERSION,
            url: "https://example.com/f.bin".into(),
            final_url: "https://example.com/f.bin".into(),
            total_size: size,
            validator: validator.map(str::to_string),
            validator_is_etag: true,
            filename: "f.bin".into(),
            target: PathBuf::from("f.bin"),
            segments: vec![SegmentSnapshot { index: 0, start: 0, end: 999, done: 100 }],
        }
    }

    #[test]
    fn resumes_when_size_and_validator_match() {
        let s = state(Some(1000), Some("\"abc\""));
        assert!(s.is_resumable_for("https://example.com/f.bin", &info(Some(1000), Some("\"abc\""), true)));
    }

    #[test]
    fn refuses_resume_when_validator_changed() {
        let s = state(Some(1000), Some("\"abc\""));
        assert!(!s.is_resumable_for("https://example.com/f.bin", &info(Some(1000), Some("\"xyz\""), true)));
    }

    #[test]
    fn refuses_resume_when_size_changed() {
        let s = state(Some(1000), Some("\"abc\""));
        assert!(!s.is_resumable_for("https://example.com/f.bin", &info(Some(2000), Some("\"abc\""), true)));
    }

    #[test]
    fn refuses_resume_when_validator_disappeared() {
        let s = state(Some(1000), Some("\"abc\""));
        assert!(!s.is_resumable_for("https://example.com/f.bin", &info(Some(1000), None, true)));
    }

    #[test]
    fn refuses_resume_without_range_support() {
        let s = state(Some(1000), Some("\"abc\""));
        assert!(!s.is_resumable_for("https://example.com/f.bin", &info(Some(1000), Some("\"abc\""), false)));
    }

    #[test]
    fn bytes_done_clamps_overshoot_from_splits() {
        let mut s = state(Some(1000), None);
        s.segments = vec![
            // done overshoots len after a split; must not inflate the total.
            SegmentSnapshot { index: 0, start: 0, end: 499, done: 700 },
            SegmentSnapshot { index: 1, start: 500, end: 999, done: 200 },
        ];
        assert_eq!(s.bytes_done(), 500 + 200);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!("fdm-state-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.part.fdm");

        let original = state(Some(1000), Some("\"abc\""));
        original.save(&path).unwrap();

        let loaded = DownloadState::load(&path).expect("should load");
        assert_eq!(loaded.total_size, Some(1000));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].done, 100);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_control_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("fdm-state-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.fdm");
        std::fs::write(&path, b"{ not json").unwrap();

        assert!(DownloadState::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
