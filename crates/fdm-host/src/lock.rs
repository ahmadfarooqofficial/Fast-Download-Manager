//! One exclusive claim per URL, so two host processes cannot write the same
//! partial file.
//!
//! This is not theoretical. Chrome starts a fresh host process every time the
//! extension's service worker restarts, and a host deliberately outlives its
//! port to finish downloads that are already running (see `main.rs`). So two
//! live hosts is the normal case, not the edge case, and if the user clicks the
//! same download twice they would both open the same `.part` file and interleave
//! positioned writes into it. The result is a file of exactly the right size
//! with the wrong bytes in it — the single failure mode this whole project is
//! built to prevent.
//!
//! The lock is the operating system's, not ours. On Windows the file is opened
//! with a share mode of zero, which makes the kernel refuse any second open
//! until this handle closes. Crucially that includes closing because the process
//! died, so there is no stale-lock problem and no need to probe whether some
//! recorded PID is still alive.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Held for as long as a download is running. Dropping it releases the claim.
pub struct UrlLock {
    path: PathBuf,
    // Never read: the value of this field is the open handle it keeps alive.
    _file: File,
}

impl Drop for UrlLock {
    fn drop(&mut self) {
        // Best effort. The handle closing is what releases the lock; removing
        // the file is only housekeeping, and it can legitimately fail if another
        // process is already opening it.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Try to claim `url`. `Ok(None)` means another live host already owns it, which
/// is a normal answer and not an error.
pub fn acquire(dir: &Path, url: &str) -> io::Result<Option<UrlLock>> {
    std::fs::create_dir_all(dir)?;

    // Hash rather than sanitise: URLs routinely exceed the 255-byte filename
    // limit and contain characters NTFS forbids, and a lossy sanitisation would
    // make two different URLs collide onto one lock.
    let path = dir.join(format!("{}.lock", hex(&Sha256::digest(url.as_bytes()))));

    match open_exclusive(&path) {
        Ok(file) => Ok(Some(UrlLock { path, _file: file })),
        Err(e) if is_contention(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether this error means "someone else holds the lock" rather than "the lock
/// is broken".
///
/// Deliberately keyed on the raw OS error code. Windows reports a share-mode
/// violation as ERROR_SHARING_VIOLATION (32), which Rust maps to
/// `ErrorKind::Uncategorized` — an unstable variant that cannot be matched and
/// is emphatically not `PermissionDenied`. Matching on `ErrorKind` here silently
/// turns normal contention into a hard error, which in this crate would mean
/// telling the user a download failed when it was merely already running.
fn is_contention(e: &io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        if matches!(
            e.raw_os_error(),
            Some(ERROR_SHARING_VIOLATION) | Some(ERROR_LOCK_VIOLATION)
        ) {
            return true;
        }
    }
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::AlreadyExists
    )
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        // dwShareMode = 0: no other process may open this file at all until the
        // handle closes. This is the entire lock.
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_exclusive(path: &Path) -> io::Result<File> {
    // FDM targets Windows, but keeping the crate buildable elsewhere is worth
    // the four lines. `create_new` is weaker — it leaves a stale lock behind if
    // the process is killed — so it is only a fallback.
    OpenOptions::new().write(true).create_new(true).open(path)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_claim_on_the_same_url_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let url = "https://example.com/big.iso";

        let first = acquire(dir.path(), url).unwrap();
        assert!(first.is_some());

        // Same URL while the first claim is held.
        assert!(
            acquire(dir.path(), url).unwrap().is_none(),
            "a second host must not be able to write the same partial file"
        );

        drop(first);
        assert!(
            acquire(dir.path(), url).unwrap().is_some(),
            "releasing the claim must make the URL available again"
        );
    }

    #[test]
    fn different_urls_do_not_contend() {
        let dir = tempfile::tempdir().unwrap();
        let a = acquire(dir.path(), "https://example.com/a.iso").unwrap();
        let b = acquire(dir.path(), "https://example.com/b.iso").unwrap();
        assert!(a.is_some() && b.is_some());
    }

    #[test]
    fn long_and_hostile_urls_still_produce_a_valid_filename() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("https://example.com/{}?a=b:c<d>e|f\"g", "x".repeat(4096));
        assert!(acquire(dir.path(), &url).unwrap().is_some());
    }
}
