//! Filename derivation and Windows-safe path handling.
//!
//! Filenames here come from a remote server, so every one of them is untrusted
//! input: path traversal, NTFS reserved device names, trailing dots that Windows
//! silently strips, and 260-character path limits all have to be handled before
//! the name touches the filesystem.

use std::path::{Path, PathBuf};

/// NTFS reserved device names. A file called `CON` or `NUL` cannot be created,
/// and the failure mode is a confusing IO error rather than an obvious one.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows forbids in a filename, plus the path separators — we only
/// ever want a bare filename here, never a relative path.
const FORBIDDEN: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Make a server-supplied string safe to use as a single filename component.
///
/// Always returns a non-empty name; falls back to `download` when nothing
/// usable survives sanitisation.
pub fn sanitize_filename(input: &str) -> String {
    // Take the last path component so `../../etc/passwd` and
    // `..\\..\\windows\\system32\\x.dll` both collapse to the leaf name.
    let leaf = input
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(input)
        .trim();

    let mut out: String = leaf
        .chars()
        .map(|c| {
            if FORBIDDEN.contains(&c) || (c as u32) < 0x20 {
                '_'
            } else {
                c
            }
        })
        .collect();

    // Windows strips trailing dots and spaces, which turns "a. " into "a" behind
    // your back and breaks any later existence check.
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }

    // A leading dot alone would make the file hidden on unix and is usually a
    // sign the name was really just an extension.
    if out.is_empty() || out == "." || out == ".." {
        return "download".to_string();
    }

    // Reserved names are matched on the stem, case-insensitively, with or
    // without an extension: both `CON` and `con.txt` are rejected by Windows.
    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }

    // Leave room for a " (99)" dedupe suffix and an extension within the
    // 255-byte per-component limit.
    if out.len() > 200 {
        let ext = Path::new(&out)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let keep = 200usize.saturating_sub(ext.len());
        out = format!("{}{}", &out[..keep.min(out.len())], ext);
    }

    out
}

/// Parse a filename out of a `Content-Disposition` header.
///
/// Handles the plain `filename="x"` form and RFC 5987 `filename*=UTF-8''x`,
/// preferring the latter because it is the one that carries non-ASCII names
/// correctly.
pub fn filename_from_content_disposition(header: &str) -> Option<String> {
    let mut plain: Option<String> = None;

    for part in header.split(';') {
        let part = part.trim();
        let (key, value) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"');

        if key == "filename*" {
            // Form: charset'language'percent-encoded-value
            let encoded = value.rsplit('\'').next().unwrap_or(value);
            if let Some(decoded) = percent_decode(encoded) {
                let name = sanitize_filename(&decoded);
                if name != "download" {
                    return Some(name);
                }
            }
        } else if key == "filename" && plain.is_none() {
            plain = Some(value.to_string());
        }
    }

    plain.map(|p| sanitize_filename(&p)).filter(|p| p != "download")
}

/// Minimal percent-decoder for `filename*` values. Returns `None` if the input
/// isn't valid UTF-8 once decoded.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            match u8::from_str_radix(hex, 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                Err(_) => return None,
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// Derive a filename from the URL path, ignoring the query string.
pub fn filename_from_url(url: &url::Url) -> String {
    let candidate = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| percent_decode(s).unwrap_or_else(|| s.to_string()))
        .unwrap_or_default();

    let name = sanitize_filename(&candidate);
    if name == "download" {
        // Better than a bare "download": give the user a hint of the origin.
        if let Some(host) = url.host_str() {
            return sanitize_filename(host);
        }
    }
    name
}

/// Resolve a collision by appending ` (2)`, ` (3)`, … before the extension —
/// the same convention Chrome and Windows Explorer use, so it looks native.
pub fn unique_path(dir: &Path, filename: &str) -> PathBuf {
    let candidate = dir.join(filename);
    if !candidate.exists() {
        return candidate;
    }

    let path = Path::new(filename);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(filename);
    let ext = path.extension().and_then(|e| e.to_str());

    for n in 2..10_000u32 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(&name);
        if !candidate.exists() {
            return candidate;
        }
    }

    // Pathological case: fall back to something guaranteed unique-ish rather
    // than looping forever.
    dir.join(format!("{stem}-{}", std::process::id()))
}

/// Opt a path into the Windows extended-length namespace so it can exceed the
/// legacy 260-character `MAX_PATH` limit. No-op on other platforms, and a no-op
/// for short or non-absolute paths.
pub fn to_extended_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if s.len() > 240 && path.is_absolute() && !s.starts_with(r"\\?\") {
            return PathBuf::from(format!(r"\\?\{s}"));
        }
    }
    path.to_path_buf()
}

/// Sidecar path holding resume state for an in-progress download.
pub fn control_path(part_path: &Path) -> PathBuf {
    let mut s = part_path.as_os_str().to_owned();
    s.push(".fdm");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_path_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename(r"..\..\windows\evil.dll"), "evil.dll");
    }

    #[test]
    fn escapes_reserved_device_names() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.txt"), "_nul.txt");
        // Not reserved — only exact stems count.
        assert_eq!(sanitize_filename("console.log"), "console.log");
    }

    #[test]
    fn removes_trailing_dots_and_spaces() {
        assert_eq!(sanitize_filename("report.pdf. "), "report.pdf");
    }

    #[test]
    fn empty_input_gets_a_usable_name() {
        assert_eq!(sanitize_filename("   "), "download");
        assert_eq!(sanitize_filename(".."), "download");
    }

    #[test]
    fn content_disposition_prefers_rfc5987() {
        let h = "attachment; filename=\"fallback.txt\"; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf";
        assert_eq!(
            filename_from_content_disposition(h).as_deref(),
            Some("résumé.pdf")
        );
    }

    #[test]
    fn content_disposition_plain_form() {
        let h = "attachment; filename=\"my file.zip\"";
        assert_eq!(
            filename_from_content_disposition(h).as_deref(),
            Some("my file.zip")
        );
    }

    #[test]
    fn url_filename_ignores_query() {
        let u = url::Url::parse("https://example.com/files/setup.exe?token=abc123").unwrap();
        assert_eq!(filename_from_url(&u), "setup.exe");
    }

    #[test]
    fn url_without_path_falls_back_to_host() {
        let u = url::Url::parse("https://example.com/").unwrap();
        assert_eq!(filename_from_url(&u), "example.com");
    }
}
