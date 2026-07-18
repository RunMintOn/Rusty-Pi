//! MIME type detection for image files via magic bytes.
//!
//! Mirrors `@earendil-works/pi-coding-agent/src/utils/mime.ts`.
//! Reads the first few bytes of a file and matches known image signatures.

/// Supported image MIME types.
pub const IMAGE_MIME_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp", "image/bmp"];

/// The number of bytes needed to sniff the MIME type.
const SNIFF_BYTES: usize = 4100;

/// Detect supported image MIME type from raw bytes.
/// Returns `None` if the bytes do not match a known image format.
pub fn detect_image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    // JPEG: starts with 0xFF 0xD8 0xFF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        // Except when byte 3 is 0xF7 (JPEG-SOF without image data)
        if bytes.len() >= 4 && bytes[3] == 0xF7 {
            return None;
        }
        return Some("image/jpeg");
    }

    // PNG: 0x89 0x50 0x4E 0x47 0x0D 0x0A 0x1A 0x0A
    if bytes.len() >= 8
        && bytes[0] == 0x89
        && bytes[1] == 0x50
        && bytes[2] == 0x4E
        && bytes[3] == 0x47
        && bytes[4] == 0x0D
        && bytes[5] == 0x0A
        && bytes[6] == 0x1A
        && bytes[7] == 0x0A
    {
        return Some("image/png");
    }

    // GIF: "GIF" at offset 0
    if bytes.len() >= 3 && bytes[0] == b'G' && bytes[1] == b'I' && bytes[2] == b'F' {
        return Some("image/gif");
    }

    // WEBP: "RIFF" at 0, "WEBP" at 8
    if bytes.len() >= 12
        && bytes[0] == b'R'
        && bytes[1] == b'I'
        && bytes[2] == b'F'
        && bytes[3] == b'F'
        && bytes[8] == b'W'
        && bytes[9] == b'E'
        && bytes[10] == b'B'
        && bytes[11] == b'P'
    {
        return Some("image/webp");
    }

    // BMP: "BM" at offset 0
    if bytes.len() >= 2 && bytes[0] == b'B' && bytes[1] == b'M' {
        return Some("image/bmp");
    }

    None
}

/// Read the first `SNIFF_BYTES` of a file and detect its image MIME type.
pub async fn detect_image_mime_type_from_file(path: &std::path::Path) -> std::io::Result<Option<&'static str>> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; SNIFF_BYTES];
    let n = file.read(&mut buf).await?;
    buf.truncate(n);
    Ok(detect_image_mime_type(&buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert_eq!(detect_image_mime_type(&bytes), Some("image/jpeg"));
    }

    #[test]
    fn detect_jpeg_exclude_sof7() {
        // JPEG start with byte 3 = 0xF7 (JPEG SOF7 — no image data)
        let bytes = [0xFF, 0xD8, 0xFF, 0xF7];
        assert_eq!(detect_image_mime_type(&bytes), None);
    }

    #[test]
    fn detect_png() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_image_mime_type(&bytes), Some("image/png"));
    }

    #[test]
    fn detect_gif() {
        let bytes = b"GIF89a...";
        assert_eq!(detect_image_mime_type(bytes), Some("image/gif"));
    }

    #[test]
    fn detect_webp() {
        // RIFF header + WEBP
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&[0u8; 4]); // size
        bytes.extend_from_slice(b"WEBP");
        assert_eq!(detect_image_mime_type(&bytes), Some("image/webp"));
    }

    #[test]
    fn detect_bmp() {
        let bytes = b"BM...";
        assert_eq!(detect_image_mime_type(bytes), Some("image/bmp"));
    }

    #[test]
    fn text_file_is_not_image() {
        let bytes = b"This is a text file.\n";
        assert_eq!(detect_image_mime_type(bytes), None);
    }

    #[test]
    fn empty_bytes_is_not_image() {
        assert_eq!(detect_image_mime_type(b""), None);
    }
}
