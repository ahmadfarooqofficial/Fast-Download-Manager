//! Decides which folder a finished download belongs in.
//!
//! Classification order is deliberate: **extension → MIME type → magic bytes**.
//! Extensions are the user's mental model so they win when present, but they
//! lie often enough that we fall back to the `Content-Type` header, and then to
//! sniffing the first bytes of the file — necessary because
//! `application/octet-stream` is served for a huge share of real downloads.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Category {
    Documents,
    Video,
    Music,
    Images,
    Compressed,
    Programs,
    Other,
}

impl Category {
    /// Subfolder name under the download root.
    pub fn folder(&self) -> &'static str {
        match self {
            Category::Documents => "Documents",
            Category::Video => "Video",
            Category::Music => "Music",
            Category::Images => "Images",
            Category::Compressed => "Compressed",
            Category::Programs => "Programs",
            Category::Other => "Other",
        }
    }

    pub const ALL: [Category; 7] = [
        Category::Documents,
        Category::Video,
        Category::Music,
        Category::Images,
        Category::Compressed,
        Category::Programs,
        Category::Other,
    ];

    /// Default extension list for a category. The real app must let the user
    /// edit these — treat this as seed data, not policy.
    pub fn default_extensions(&self) -> &'static [&'static str] {
        match self {
            Category::Documents => &[
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "rtf", "odt", "ods",
                "odp", "csv", "epub", "mobi", "azw3", "djvu", "tex", "md",
            ],
            Category::Video => &[
                "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts",
                "m2ts", "vob", "3gp", "ogv",
            ],
            Category::Music => &[
                "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "aiff", "alac", "mid",
                "midi",
            ],
            Category::Images => &[
                "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "tif", "tiff", "heic", "heif",
                "avif", "ico", "psd", "raw", "cr2", "nef",
            ],
            Category::Compressed => &[
                "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "zst", "iso", "img", "cab", "arj",
                "lz", "lzma", "tgz",
            ],
            Category::Programs => &[
                "exe", "msi", "msix", "appx", "bat", "cmd", "ps1", "apk", "deb", "rpm", "dmg",
                "pkg", "appimage", "jar",
            ],
            Category::Other => &[],
        }
    }
}

/// Classify by file extension alone. `None` when the extension is unknown.
pub fn from_extension(ext: &str) -> Option<Category> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if ext.is_empty() {
        return None;
    }
    Category::ALL
        .iter()
        .find(|c| c.default_extensions().contains(&ext.as_str()))
        .copied()
}

/// Classify by `Content-Type`. Handles both the top-level type and a few
/// specific subtypes that don't follow the top-level grouping.
pub fn from_mime(mime: &str) -> Option<Category> {
    // Strip parameters: "text/plain; charset=utf-8" -> "text/plain"
    let mime = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if mime.is_empty() || mime == "application/octet-stream" || mime == "binary/octet-stream" {
        return None;
    }

    // Specific subtypes first — these would be misfiled by top-level matching.
    let specific = match mime.as_str() {
        "application/pdf"
        | "application/msword"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-powerpoint"
        | "application/rtf"
        | "application/epub+zip" => Some(Category::Documents),

        "application/zip" | "application/x-zip-compressed" | "application/x-rar-compressed"
        | "application/vnd.rar" | "application/x-7z-compressed" | "application/gzip"
        | "application/x-gzip" | "application/x-tar" | "application/x-bzip2"
        | "application/x-xz" | "application/x-iso9660-image" => Some(Category::Compressed),

        "application/vnd.microsoft.portable-executable"
        | "application/x-msdownload"
        | "application/x-msi"
        | "application/vnd.android.package-archive"
        | "application/x-apple-diskimage"
        | "application/java-archive" => Some(Category::Programs),

        "application/x-mpegurl" | "application/vnd.apple.mpegurl" | "application/dash+xml" => {
            Some(Category::Video)
        }

        "image/svg+xml" => Some(Category::Images),
        _ => None,
    };
    if specific.is_some() {
        return specific;
    }

    // OOXML types all start with this prefix; the tail decides which app.
    if let Some(tail) = mime.strip_prefix("application/vnd.openxmlformats-officedocument.") {
        let _ = tail;
        return Some(Category::Documents);
    }

    match mime.split('/').next()? {
        "video" => Some(Category::Video),
        "audio" => Some(Category::Music),
        "image" => Some(Category::Images),
        "text" => Some(Category::Documents),
        _ => None,
    }
}

/// Last resort: identify by file signature. `head` should be the first ~16
/// bytes of the file, which we already have from segment 0.
pub fn from_magic(head: &[u8]) -> Option<Category> {
    if head.len() < 4 {
        return None;
    }
    let starts = |sig: &[u8]| head.len() >= sig.len() && &head[..sig.len()] == sig;

    // ISO base media (mp4/mov/m4a/heic) puts the brand at offset 4.
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        return Some(match brand {
            b"M4A " | b"M4B " => Category::Music,
            b"heic" | b"heix" | b"mif1" | b"avif" => Category::Images,
            _ => Category::Video,
        });
    }

    // RIFF containers: the form type at offset 8 disambiguates WAV from AVI.
    if starts(b"RIFF") && head.len() >= 12 {
        return Some(match &head[8..12] {
            b"WAVE" => Category::Music,
            b"AVI " => Category::Video,
            b"WEBP" => Category::Images,
            _ => Category::Other,
        });
    }

    if starts(b"%PDF") {
        return Some(Category::Documents);
    }
    if starts(b"PK\x03\x04") {
        // Could be an OOXML document or a plain zip. Without reading the central
        // directory we can't tell, so treat it as an archive.
        return Some(Category::Compressed);
    }
    if starts(b"Rar!\x1a\x07")
        || starts(b"7z\xbc\xaf\x27\x1c")
        || starts(b"\x1f\x8b")
        || starts(b"\xfd7zXZ\x00")
        || starts(b"BZh")
        || starts(b"\x28\xb5\x2f\xfd")
    {
        return Some(Category::Compressed);
    }
    if starts(b"\x89PNG") || starts(b"\xff\xd8\xff") || starts(b"GIF8") || starts(b"BM") {
        return Some(Category::Images);
    }
    if starts(b"ID3") || starts(b"fLaC") || starts(b"OggS") || starts(b"\xff\xfb") {
        return Some(Category::Music);
    }
    if starts(b"\x1a\x45\xdf\xa3") {
        return Some(Category::Video); // Matroska / WebM
    }
    if starts(b"MZ") || starts(b"\xd0\xcf\x11\xe0") || starts(b"\x7fELF") {
        return Some(Category::Programs);
    }
    None
}

/// Full classification chain. `head` may be empty if no bytes are available yet.
pub fn classify(filename: &str, mime: Option<&str>, head: &[u8]) -> Category {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    from_extension(ext)
        .or_else(|| mime.and_then(from_mime))
        .or_else(|| from_magic(head))
        .unwrap_or(Category::Other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_wins_over_mime() {
        // Servers routinely mislabel installers as octet-stream.
        assert_eq!(
            classify("setup.exe", Some("application/octet-stream"), &[]),
            Category::Programs
        );
    }

    #[test]
    fn mime_used_when_extension_missing() {
        assert_eq!(classify("download", Some("video/mp4"), &[]), Category::Video);
    }

    #[test]
    fn magic_used_when_extension_and_mime_useless() {
        assert_eq!(
            classify("download", Some("application/octet-stream"), b"%PDF-1.7"),
            Category::Documents
        );
    }

    #[test]
    fn riff_form_type_disambiguates() {
        let mut wav = Vec::from(*b"RIFF");
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVE");
        assert_eq!(from_magic(&wav), Some(Category::Music));

        let mut avi = Vec::from(*b"RIFF");
        avi.extend_from_slice(&[0, 0, 0, 0]);
        avi.extend_from_slice(b"AVI ");
        assert_eq!(from_magic(&avi), Some(Category::Video));
    }

    #[test]
    fn unknown_falls_through_to_other() {
        assert_eq!(classify("mystery", None, &[]), Category::Other);
    }

    #[test]
    fn ooxml_is_a_document_not_an_archive() {
        assert_eq!(
            from_mime("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            Some(Category::Documents)
        );
    }
}
