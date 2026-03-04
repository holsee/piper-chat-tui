//! Shared utility functions.

use crate::protocol::MessageId;

/// Generate a new random 128-bit message ID.
pub fn new_message_id() -> MessageId {
    rand::random()
}

/// Current wall-clock time as milliseconds since UNIX epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Format a byte count as a human-readable file size string.
pub fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Infer a MIME type from a filename extension.
/// Returns `None` for unrecognized extensions.
pub fn mime_from_extension(filename: &str) -> Option<String> {
    let ext = filename.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        "png" => Some("image/png".into()),
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "gif" => Some("image/gif".into()),
        "webp" => Some("image/webp".into()),
        "mp4" => Some("video/mp4".into()),
        "webm" => Some("video/webm".into()),
        "mov" => Some("video/quicktime".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_file_size_units() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(512), "512 B");
        assert_eq!(format_file_size(1024), "1.0 KB");
        assert_eq!(format_file_size(1536), "1.5 KB");
        assert_eq!(format_file_size(1048576), "1.0 MB");
        assert_eq!(format_file_size(1073741824), "1.0 GB");
    }

    #[test]
    fn mime_from_extension_images() {
        assert_eq!(mime_from_extension("photo.png"), Some("image/png".into()));
        assert_eq!(mime_from_extension("pic.jpg"), Some("image/jpeg".into()));
        assert_eq!(
            mime_from_extension("pic.JPEG"),
            Some("image/jpeg".into())
        );
        assert_eq!(mime_from_extension("anim.gif"), Some("image/gif".into()));
        assert_eq!(
            mime_from_extension("img.webp"),
            Some("image/webp".into())
        );
    }

    #[test]
    fn mime_from_extension_videos() {
        assert_eq!(mime_from_extension("clip.mp4"), Some("video/mp4".into()));
        assert_eq!(
            mime_from_extension("vid.webm"),
            Some("video/webm".into())
        );
        assert_eq!(
            mime_from_extension("movie.mov"),
            Some("video/quicktime".into())
        );
    }

    #[test]
    fn mime_from_extension_unknown() {
        assert_eq!(mime_from_extension("doc.txt"), None);
        assert_eq!(mime_from_extension("archive.zip"), None);
        assert_eq!(mime_from_extension("noext"), None);
    }
}
