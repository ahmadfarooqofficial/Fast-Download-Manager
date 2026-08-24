//! Persistence for the download list.
//!
//! The list survives a restart, but only as far as it honestly can: the URL, the
//! destination and the byte count are saved; the browser headers are not. See
//! [`crate::Manager`] for why.
//!
//! Writes are atomic (temp file + rename) because the alternative is a truncated
//! JSON file after a power cut, which loses the *entire* list rather than one
//! download.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{DownloadEntry, DownloadId, Status};

/// Bumped when the on-disk shape changes incompatibly. A file from the future is
/// discarded rather than guessed at — the same policy as `fdm-core`'s `.fdm`
/// control file, for the same reason.
const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    next_id: DownloadId,
    entries: Vec<DownloadEntry>,
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `%LOCALAPPDATA%\FDM\downloads.json` on Windows.
    ///
    /// LocalAppData, not Roaming: a half-finished download refers to a `.part`
    /// file on this machine's disk, so syncing the list to another machine would
    /// only produce rows that can never resume.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| dirs_home().map(|h| h.join("AppData\\Local")))
            .unwrap_or_else(std::env::temp_dir);
        base.join("FDM").join("downloads.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the list back. Returns `(next_id, entries)`.
    ///
    /// Anything that was mid-flight when the process died is rewritten as
    /// `Paused`: nothing is running, so reporting `Downloading` would show a
    /// frozen speed and a stop button that stops nothing. Paused is both true and
    /// actionable — the `.fdm` control file is still there to resume from.
    pub fn load(&self) -> (DownloadId, Vec<DownloadEntry>) {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %self.path.display(), error = %e, "could not read the download list");
                }
                return (1, Vec::new());
            }
        };

        let snap: Snapshot = match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "download list is unreadable; starting empty");
                return (1, Vec::new());
            }
        };

        if snap.version != FORMAT_VERSION {
            tracing::warn!(
                found = snap.version,
                expected = FORMAT_VERSION,
                "download list has an unsupported version; starting empty"
            );
            return (1, Vec::new());
        }

        let mut entries = snap.entries;
        for e in &mut entries {
            if e.status.is_active() {
                e.status = Status::Paused;
                e.speed_bps = 0.0;
                e.active_connections = 0;
            }
        }

        let highest = entries.iter().map(|e| e.id).max().unwrap_or(0);
        (snap.next_id.max(highest + 1), entries)
    }

    /// Overwrite the list. Called on state transitions only, never on progress —
    /// four writes a second per download would be pointless churn, and the byte
    /// count is recoverable from the `.fdm` control file anyway.
    pub fn save(&self, next_id: DownloadId, entries: &[DownloadEntry]) {
        if let Err(e) = self.try_save(next_id, entries) {
            // A failed save must not take a download with it. The user cares
            // about the file, not the bookkeeping.
            tracing::warn!(path = %self.path.display(), error = %e, "could not save the download list");
        }
    }

    fn try_save(&self, next_id: DownloadId, entries: &[DownloadEntry]) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }

        let snap = Snapshot {
            version: FORMAT_VERSION,
            next_id,
            entries: entries.to_vec(),
        };
        let json = serde_json::to_vec_pretty(&snap)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Same directory, so the rename is on one volume and therefore atomic.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        // Windows `rename` refuses to clobber, unlike POSIX. `std::fs::rename`
        // uses MoveFileEx with MOVEFILE_REPLACE_EXISTING and does replace, but a
        // stale temp file from a previous crash would still be in the way of the
        // *write* above if it were read-only, so this stays explicit and simple.
        std::fs::rename(&tmp, &self.path)
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: DownloadId, status: Status) -> DownloadEntry {
        let mut e = DownloadEntry::new(id, "https://example.com/f.bin", "f.bin".into());
        e.status = status;
        e
    }

    #[test]
    fn round_trips_a_list() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("downloads.json"));

        let mut done = entry(1, Status::Completed);
        done.downloaded = 4096;
        done.total = Some(4096);
        store.save(9, &[done]);

        let (next, back) = store.load();
        assert_eq!(next, 9);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].status, Status::Completed);
        assert_eq!(back[0].downloaded, 4096);
    }

    #[test]
    fn a_missing_file_is_an_empty_list_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (next, entries) = Store::new(dir.path().join("nope.json")).load();
        assert_eq!(next, 1);
        assert!(entries.is_empty());
    }

    #[test]
    fn interrupted_downloads_come_back_as_paused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("downloads.json"));

        let mut running = entry(3, Status::Downloading);
        running.speed_bps = 5_000_000.0;
        running.active_connections = 8;
        store.save(4, &[running, entry(4, Status::Queued), entry(5, Status::Completed)]);

        let (_, back) = store.load();
        // Nothing is running after a restart, so nothing may claim to be.
        assert_eq!(back[0].status, Status::Paused);
        assert_eq!(back[0].speed_bps, 0.0);
        assert_eq!(back[0].active_connections, 0);
        assert_eq!(back[1].status, Status::Paused);
        // A finished download is untouched.
        assert_eq!(back[2].status, Status::Completed);
    }

    #[test]
    fn next_id_can_never_collide_with_a_saved_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("downloads.json"));
        // A next_id that contradicts the entries — hand-edited file, or a bug.
        store.save(2, &[entry(7, Status::Completed)]);
        let (next, _) = store.load();
        assert_eq!(next, 8, "reusing id 2..7 would overwrite an existing row");
    }

    #[test]
    fn a_corrupt_file_starts_empty_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.json");
        std::fs::write(&path, b"{not json at all").unwrap();
        let (next, entries) = Store::new(&path).load();
        assert_eq!(next, 1);
        assert!(entries.is_empty());
    }

    #[test]
    fn a_file_from_a_future_version_is_discarded_not_guessed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downloads.json");
        std::fs::write(&path, br#"{"version":999,"next_id":5,"entries":[]}"#).unwrap();
        let (next, entries) = Store::new(&path).load();
        assert_eq!(next, 1);
        assert!(entries.is_empty());
    }
}
