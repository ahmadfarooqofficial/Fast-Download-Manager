//! Positioned file writes.
//!
//! Every segment worker holds a clone of the same handle and writes directly at
//! its absolute byte offset. This is the design decision that lets us finish the
//! moment the last byte lands — IDM downloads segments to separate parts and
//! then spends real time merging them, which on a 10 GB file is a visible wait.
//!
//! `seek_write` on Windows and `write_at` on unix are both positional: they take
//! `&self` and do not disturb a shared file cursor, so concurrent writes from
//! many workers to one handle are safe.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

#[cfg(windows)]
use std::os::windows::fs::FileExt;
#[cfg(unix)]
use std::os::unix::fs::FileExt;

pub struct PositionedFile {
    file: File,
}

impl PositionedFile {
    /// Open (creating if needed) the target file and preallocate it to `size`.
    ///
    /// Preallocation matters for two reasons: writing at high offsets into a
    /// zero-length file forces the filesystem to extend it repeatedly, and a
    /// known final size lets NTFS pick contiguous extents instead of scattering
    /// the file across the disk.
    pub fn create(path: &Path, size: Option<u64>) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        if let Some(size) = size {
            // Only resize when it differs, so resuming an existing .part file
            // doesn't discard data.
            if file.metadata()?.len() != size {
                file.set_len(size)?;
            }
        }

        Ok(Self { file })
    }

    /// Write the whole buffer at `offset`, looping on short writes.
    pub fn write_all_at(&self, buf: &[u8], offset: u64) -> io::Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            let n = self.write_at(&buf[written..], offset + written as u64)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "positioned write made no progress",
                ));
            }
            written += n;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.file.seek_write(buf, offset)
    }

    #[cfg(unix)]
    fn write_at(&self, buf: &[u8], offset: u64) -> io::Result<usize> {
        self.file.write_at(buf, offset)
    }

    /// Read back bytes at an offset — used to sniff the file signature for
    /// categorisation once segment 0 has landed.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> io::Result<usize> {
        #[cfg(windows)]
        {
            self.file.seek_read(buf, offset)
        }
        #[cfg(unix)]
        {
            self.file.read_at(buf, offset)
        }
    }

    /// Flush file data to disk. Called at checkpoints, not per write — a sync on
    /// every chunk would destroy throughput.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Current size of the file on disk, including any preallocated tail.
    ///
    /// Not `len`: a fallible size query reads as a collection length at the call
    /// site, and callers ask this to compare against the *expected* total, not to
    /// find out whether anything is in there.
    pub fn size_on_disk(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    /// Trim the file to its true length. Needed when the final size wasn't known
    /// up front and preallocation overshot.
    pub fn truncate_to(&self, size: u64) -> io::Result<()> {
        self.file.set_len(size)
    }
}
