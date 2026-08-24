//! Where partial downloads live, and how a finished one reaches its destination.
//!
//! By default the `.part` file and its `.fdm` control file are written to a
//! dedicated temporary directory rather than next to the finished file. That is
//! what IDM does, and it buys three things:
//!
//! - the download folder never shows half-finished files, so "is it done?" is
//!   answered by the file simply being there;
//! - a `.part` cannot be mistaken for the download and opened early;
//! - the scratch data can live on a fast local disk while the finished file goes
//!   to a slow or removable one.
//!
//! The cost is that finishing may now cross a volume boundary, which `rename`
//! cannot do — see [`move_into_place`].
//!
//! ## Why the scratch name is a hash
//!
//! Resume has to find the same `.part` file on the next run, so the name must be
//! derived from the download's identity rather than allocated. It hashes the URL,
//! the destination directory and the *requested* filename — not the final
//! de-duplicated path, because `unique_path` may append " (2)" and that would
//! change the name between attempts and orphan the partial data.

use std::path::{Path, PathBuf};

use crate::naming;

/// Longest readable prefix kept in a scratch filename. Enough to recognise a
/// download in the temp folder without risking the 255-character NTFS component
/// limit once the hash and extension are added.
const STEM_LIMIT: usize = 48;

/// Path of the `.part` file for one download inside `temp_dir`.
///
/// `dir` and `filename` are the *intended* destination, before de-duplication.
pub fn part_path(temp_dir: &Path, url: &str, dir: &Path, filename: &str) -> PathBuf {
    let mut key = String::with_capacity(url.len() + filename.len() + 8);
    key.push_str(url);
    key.push('\u{0}');
    key.push_str(&dir.to_string_lossy());
    key.push('\u{0}');
    key.push_str(filename);

    let stem = readable_stem(filename);
    temp_dir.join(format!("{stem}-{:016x}.part", fnv1a64(key.as_bytes())))
}

/// A short, safe, recognisable version of the filename for the scratch name.
fn readable_stem(filename: &str) -> String {
    let safe = naming::sanitize_filename(filename);
    let stem = Path::new(&safe)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| safe.clone());

    let mut out: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(STEM_LIMIT)
        .collect();

    // A leading or trailing dot or space is legal in the string but not on NTFS,
    // and an empty stem would produce a name that is only a hash.
    let trimmed = out.trim_matches(|c: char| c == '.' || c == ' ' || c == '_');
    if trimmed.is_empty() {
        out = "download".to_string();
    } else {
        out = trimmed.to_string();
    }
    out
}

/// FNV-1a, 64-bit.
///
/// Chosen over `DefaultHasher` because the value has to mean the same thing on
/// the next run of a possibly newer build: `DefaultHasher`'s output is explicitly
/// not stable across Rust releases, and a changed hash silently orphans every
/// partial download on the user's disk.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Move the finished `.part` file onto its final path.
///
/// `rename` is one atomic metadata update when both paths are on the same volume,
/// and fails outright when they are not — which is now the normal case, because
/// the temp directory defaults to `%LOCALAPPDATA%` while downloads may go to any
/// drive. Only that specific failure falls back to copy-then-delete; a permission
/// error still surfaces as a permission error rather than as a slow copy that
/// fails for the same reason.
pub fn move_into_place(part: &Path, target: &Path) -> std::io::Result<()> {
    let from = naming::to_extended_path(part);
    let to = naming::to_extended_path(target);

    match std::fs::rename(&from, &to) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_volume(&e) => {
            tracing::debug!(
                from = %part.display(),
                to = %target.display(),
                "temp and destination are on different volumes; copying"
            );
            std::fs::copy(&from, &to)?;
            // The copy is the download now. A failure to unlink the source leaves
            // litter in the temp folder, which is worth a warning and nothing more
            // — reporting it as a download failure would be a lie.
            if let Err(e) = std::fs::remove_file(&from) {
                tracing::warn!(path = %part.display(), error = %e, "could not remove the temp file after copying");
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Windows reports a cross-volume rename as `ERROR_NOT_SAME_DEVICE` (17), Unix as
/// `EXDEV` (18). Keyed on the raw code for the same reason `lock.rs` is: the
/// `ErrorKind` that covers this (`CrossesDevices`) is not available on the
/// toolchain this crate supports.
fn is_cross_volume(e: &std::io::Error) -> bool {
    #[cfg(windows)]
    const CODE: i32 = 17;
    #[cfg(not(windows))]
    const CODE: i32 = 18;
    e.raw_os_error() == Some(CODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMP: &str = r"C:\Users\me\AppData\Local\FDM\Temp";
    const DIR: &str = r"C:\Users\me\Downloads\FDM\Compressed";

    #[test]
    fn the_same_download_always_gets_the_same_scratch_file() {
        // This is the property resume depends on. If it ever fails, every
        // in-progress download on every user's disk is orphaned by an upgrade.
        let a = part_path(Path::new(TEMP), "https://x.test/a.zip", Path::new(DIR), "a.zip");
        let b = part_path(Path::new(TEMP), "https://x.test/a.zip", Path::new(DIR), "a.zip");
        assert_eq!(a, b);
    }

    #[test]
    fn the_hash_value_is_pinned() {
        // Pinned deliberately: changing the hash is a breaking change to
        // on-disk state, and this test is what makes that visible in review
        // rather than in a bug report about lost downloads.
        assert_eq!(fnv1a64(b"fdm"), 0xdcca_7718_feee_6abe);
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn different_urls_with_one_filename_do_not_share_a_part_file() {
        let a = part_path(Path::new(TEMP), "https://a.test/f.zip", Path::new(DIR), "f.zip");
        let b = part_path(Path::new(TEMP), "https://b.test/f.zip", Path::new(DIR), "f.zip");
        assert_ne!(a, b, "two downloads writing one .part file is corruption");
    }

    #[test]
    fn the_same_url_to_two_folders_does_not_share_a_part_file() {
        let a = part_path(Path::new(TEMP), "https://x.test/f.zip", Path::new(r"C:\one"), "f.zip");
        let b = part_path(Path::new(TEMP), "https://x.test/f.zip", Path::new(r"C:\two"), "f.zip");
        assert_ne!(a, b);
    }

    #[test]
    fn the_name_stays_recognisable() {
        let p = part_path(
            Path::new(TEMP),
            "https://x.test/d",
            Path::new(DIR),
            "Ubuntu 24.04 Desktop.iso",
        );
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("Ubuntu_24"), "got {name}");
        assert!(name.ends_with(".part"), "got {name}");
    }

    #[test]
    fn a_hostile_filename_still_produces_one_valid_component() {
        let p = part_path(
            Path::new(TEMP),
            "https://x.test/d",
            Path::new(DIR),
            "../../windows/system32/evil.dll",
        );
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(!name.contains('/') && !name.contains('\\'), "got {name}");
        assert!(!name.contains(".."), "got {name}");
        assert_eq!(p.parent().unwrap(), Path::new(TEMP));
    }

    #[test]
    fn a_filename_with_nothing_usable_in_it_still_works() {
        let p = part_path(Path::new(TEMP), "https://x.test/d", Path::new(DIR), "...");
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("download-"), "got {name}");
    }

    #[test]
    fn a_very_long_name_stays_inside_the_ntfs_component_limit() {
        let long = format!("{}.bin", "x".repeat(400));
        let p = part_path(Path::new(TEMP), "https://x.test/d", Path::new(DIR), &long);
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.len() <= STEM_LIMIT + 22, "got {} chars", name.len());
    }

    #[test]
    fn a_same_volume_move_is_a_rename() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("x.part");
        let target = dir.path().join("x.bin");
        std::fs::write(&part, b"payload").unwrap();

        move_into_place(&part, &target).unwrap();

        assert!(!part.exists(), "the temp file must not survive");
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
    }

    #[test]
    fn a_real_failure_is_not_swallowed_by_the_copy_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.part");
        let err = move_into_place(&missing, &dir.path().join("out.bin")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
