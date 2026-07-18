//! Read tool — read file contents.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/read.ts`.
//! Supports text files with offset/limit, and image files (jpg, png, gif, webp, bmp).
//! On macOS, applies NFD / AM-PM / curly-quote path fallbacks for files
//! whose user-typed path doesn't match the filesystem's decomposed form.

use crate::agent::types::{AgentTool, AgentToolResult};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::mime::detect_image_mime_type_from_file;
use crate::coding_agent::tools::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Parameters for the read tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// Path to the file to read (relative or absolute).
    pub path: String,
    /// Line number to start reading from (1-indexed).
    pub offset: Option<usize>,
    /// Maximum number of lines to read.
    pub limit: Option<usize>,
}

/// Resolve a path relative to the current working directory.
fn resolve_to_cwd(file_path: &str, cwd: &str) -> String {
    let path = Path::new(file_path);
    if path.is_absolute() {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_string()
    } else {
        let joined = Path::new(cwd).join(path);
        std::fs::canonicalize(&joined)
            .unwrap_or(joined)
            .to_string_lossy()
            .to_string()
    }
}

// ── macOS path tolerance ────────────────────────────────────────────────────

/// Try macOS AM/PM screenshot variant: narrow no-break space before AM/PM.
fn try_am_pm_variant(file_path: &str) -> String {
    file_path.replace(" AM.", "\u{202F}AM.").replace(" PM.", "\u{202F}PM.")
}

/// Try NFD (decomposed) variant — macOS stores filenames in NFD form.
fn try_nfd_variant(file_path: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    file_path.nfd().collect()
}

/// Try curly-quote variant — macOS uses U+2019 in screenshot names like "Capture d'écran".
fn try_curly_quote_variant(file_path: &str) -> String {
    file_path.replace('\'', "\u{2019}")
}

/// Resolve a read path, trying macOS tolerance fallbacks if the exact path doesn't exist.
fn resolve_read_path(file_path: &str, cwd: &str) -> String {
    let resolved = resolve_to_cwd(file_path, cwd);
    let path = Path::new(&resolved);

    if path.exists() {
        return resolved;
    }

    // Try AM/PM variant (macOS uses NNBSP before AM/PM in screenshot names)
    let am_pm = try_am_pm_variant(&resolved);
    if am_pm != resolved && Path::new(&am_pm).exists() {
        return am_pm;
    }

    // Try NFD variant
    let nfd = try_nfd_variant(&resolved);
    if nfd != resolved && Path::new(&nfd).exists() {
        return nfd;
    }

    // Try curly-quote variant
    let curly = try_curly_quote_variant(&resolved);
    if curly != resolved && Path::new(&curly).exists() {
        return curly;
    }

    // Try combined NFD + curly-quote (for French macOS screenshots)
    let nfd_curly = try_curly_quote_variant(&nfd);
    if nfd_curly != resolved && Path::new(&nfd_curly).exists() {
        return nfd_curly;
    }

    resolved
}

// ── Image handling ──────────────────────────────────────────────────────────

/// Base64-encode raw image bytes and wrap in an Image content block.
async fn read_image(path: &Path) -> anyhow::Result<Vec<Content>> {
    use base64::Engine;

    let bytes = tokio::fs::read(path).await?;
    let mime = detect_image_mime_type_from_file(path)
        .await?
        .unwrap_or("image/jpeg");
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);

    Ok(vec![Content::Image {
        data: encoded,
        mime_type: mime.to_string(),
    }])
}

// ── Tool implementation ─────────────────────────────────────────────────────

/// The read tool — reads file contents.
pub struct ReadTool {
    shared_cwd: Arc<RwLock<PathBuf>>,
}

impl ReadTool {
    pub fn new(shared_cwd: Arc<RwLock<PathBuf>>) -> Self {
        Self { shared_cwd }
    }

    fn cwd(&self) -> String {
        self.shared_cwd.read().unwrap().to_string_lossy().to_string()
    }
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Supports text files and images (jpg, png, gif, webp, bmp). \
For text files, output is truncated to 2000 lines or 50KB (whichever is hit first). \
Use offset/limit for large files. When you need the full file, continue with offset until complete."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative or absolute)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["path"]
        })
    }
}

#[async_trait]
impl AgentTool for ReadTool {
    fn label(&self) -> &str {
        "read"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult> {
        let read_params: ReadParams = serde_json::from_value(params)
            .map_err(|e| anyhow::anyhow!("Invalid read parameters: {}", e))?;

        // Check abort signal
        if let Some(rx) = &signal
            && *rx.borrow() {
                return Ok(AgentToolResult {
                    content: vec![Content::Text { text: "Operation aborted".into() }],
                    details: serde_json::json!({"aborted": true}),
                    ..Default::default()
                });
            }

        // Resolve path with macOS tolerance fallbacks
        let absolute_path = resolve_read_path(&read_params.path, &self.cwd());
        let path = Path::new(&absolute_path);

        // Check if file exists (resolve_read_path already tried fallbacks,
        // but we still need to error if nothing matched)
        if !path.exists() {
            anyhow::bail!("File not found: {}", read_params.path);
        }

        // Check abort after IO
        if let Some(rx) = &signal
            && *rx.borrow() {
                return Ok(AgentToolResult {
                    content: vec![Content::Text { text: "Operation aborted".into() }],
                    details: serde_json::json!({"aborted": true}),
                    ..Default::default()
                });
            }

        // Detect image MIME type by sniffing magic bytes
        let mime = detect_image_mime_type_from_file(path).await
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", read_params.path, e))?;

        if let Some(mime_type) = mime {
            // Image file — read as bytes, base64-encode, return Image content
            let content = read_image(path).await?;
            return Ok(AgentToolResult {
                content,
                details: serde_json::json!({"mime_type": mime_type}),
                ..Default::default()
            });
        }

        // Text file — read as string
        let file_content = tokio::fs::read_to_string(path).await
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", read_params.path, e))?;

        let all_lines: Vec<&str> = file_content.split('\n').collect();
        let total_file_lines = all_lines.len();

        // Apply offset (1-indexed)
        let start_line = read_params.offset.map(|o| o.saturating_sub(1)).unwrap_or(0);
        if start_line >= all_lines.len() {
            anyhow::bail!(
                "Offset {} is beyond end of file ({} lines total)",
                read_params.offset.unwrap_or(0),
                total_file_lines
            );
        }

        // Slice by offset/limit
        let selected_content: String = if let Some(limit) = read_params.limit {
            let end = std::cmp::min(start_line + limit, all_lines.len());
            all_lines[start_line..end].join("\n")
        } else {
            all_lines[start_line..].join("\n")
        };

        // Apply head truncation
        let tr = truncate_head(&selected_content, DEFAULT_MAX_LINES, DEFAULT_MAX_BYTES);

        let mut result_text = tr.content;

        // Build continuation hints
        if tr.first_line_exceeds_limit {
            let first_line_size = selected_content.lines().next().map(|l| l.len()).unwrap_or(0);
            result_text = format!(
                "[Line {} is {}, exceeds {} limit. Use bash: sed -n '{}p' {} | head -c {}]",
                start_line + 1,
                format_size(first_line_size),
                format_size(DEFAULT_MAX_BYTES),
                start_line + 1,
                read_params.path,
                DEFAULT_MAX_BYTES
            );
        } else if tr.truncated {
            let end_line_display = start_line + tr.output_lines;
            let next_offset = end_line_display + 1;
            if tr.truncated_by == "lines" {
                result_text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    start_line + 1,
                    end_line_display,
                    total_file_lines,
                    next_offset
                ));
            } else {
                result_text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} limit). Use offset={} to continue.]",
                    start_line + 1,
                    end_line_display,
                    total_file_lines,
                    format_size(DEFAULT_MAX_BYTES),
                    next_offset
                ));
            }
        } else if let Some(limit) = read_params.limit {
            let remaining = total_file_lines.saturating_sub(start_line + limit);
            if remaining > 0 {
                let next_offset = start_line + limit + 1;
                result_text.push_str(&format!(
                    "\n\n[{} more lines in file. Use offset={} to continue.]",
                    remaining, next_offset
                ));
            }
        }

        Ok(AgentToolResult {
            content: vec![Content::Text { text: result_text }],
            details: serde_json::json!({"total_lines": total_file_lines}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    fn tool() -> ReadTool {
        let shared_cwd = Arc::new(RwLock::new(
            std::env::current_dir().unwrap()
        ));
        ReadTool::new(shared_cwd)
    }

    async fn tool_and_temp() -> (ReadTool, Arc<TokioMutex<tempfile::TempDir>>) {
        let tmp = tempfile::tempdir().unwrap();
        let shared_cwd = Arc::new(RwLock::new(tmp.path().to_path_buf()));
        let tool = ReadTool::new(shared_cwd);
        let tmp_arc = Arc::new(TokioMutex::new(tmp));
        (tool, tmp_arc)
    }

    #[tokio::test]
    async fn read_existing_file() {
        let result = tool()
            .execute("c1", serde_json::json!({"path": "src/coding_agent/tools/read.rs"}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Read tool"), "Should contain file content, got: {}", text);
        assert!(result.details["total_lines"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn read_with_offset() {
        let result = tool()
            .execute("c2", serde_json::json!({"path": "src/coding_agent/tools/read.rs", "offset": 1}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Read tool"), "Should contain file content from line 1");
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let result = tool()
            .execute(
                "c3",
                serde_json::json!({"path": "src/coding_agent/tools/read.rs", "offset": 1, "limit": 5}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Read tool"), "Should contain text from line 1");
        let lines: Vec<&str> = text.split('\n').collect();
        assert!(lines.len() <= 8, "Should have at most 5 content lines + continuation (got {})", lines.len());
    }

    #[tokio::test]
    async fn read_file_not_found() {
        let result = tool()
            .execute("c4", serde_json::json!({"path": "nonexistent_file.txt"}), None)
            .await;
        assert!(result.is_err(), "Should error on nonexistent file");
    }

    #[tokio::test]
    async fn read_offset_beyond_end() {
        let result = tool()
            .execute("c5", serde_json::json!({"path": "src/coding_agent/tools/read.rs", "offset": 999999}), None)
            .await;
        assert!(result.is_err(), "Should error on offset beyond end of file");
    }

    #[tokio::test]
    async fn read_abort() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let tool_instance = tool();

        let handle = tokio::spawn(async move {
            tool_instance
                .execute("c6", serde_json::json!({"path": "src/coding_agent/tools/read.rs"}), Some(rx))
                .await
        });

        tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("aborted"), "Expected aborted message, got: {}", text);
    }

    // ── Image support ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_image_png() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        // Write a minimal 1x1 red PNG (67 bytes)
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // chunk length: 13
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1 pixel
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, // color type + CRC
            0x00, 0x00, 0x00, 0x0C, // chunk length: 12
            0x49, 0x44, 0x41, 0x54, // "IDAT"
            0x08, 0xD7, 0x63, 0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x27, 0x34, 0x27, // compressed data + CRC
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // "IEND"
            0xAE, 0x42, 0x60, 0x82, // CRC
        ];

        let file_path = std::path::Path::new(&dir).join("test.png");
        tokio::fs::write(&file_path, &png_bytes).await.unwrap();

        let result = tool
            .execute("c_img", serde_json::json!({"path": "test.png"}), None)
            .await
            .unwrap();

        assert_eq!(result.content.len(), 1, "Should have one content block");
        match &result.content[0] {
            Content::Image { data, mime_type } => {
                assert_eq!(mime_type, "image/png");
                // Verify the data is valid base64
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
                assert_eq!(decoded.len(), png_bytes.len(), "Decoded image should match original size");
            }
            other => panic!("Expected Image content, got {:?}", other),
        }
        assert_eq!(result.details["mime_type"], "image/png");
    }

    #[tokio::test]
    async fn read_image_jpeg() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        // Minimal valid JPEG (starts with FF D8 FF E0)
        let jpeg_bytes: Vec<u8> = vec![
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
            0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43,
            0x00, 0x08, 0x06, 0x06, 0x07, 0x06, 0x05, 0x08, 0x07, 0x07, 0x07, 0x09,
            0x09, 0x08, 0x0A, 0x0C, 0x14, 0x0D, 0x0C, 0x0B, 0x0B, 0x0C, 0x19, 0x12,
            0x13, 0x0F, 0x14, 0x1D, 0x1A, 0x1F, 0x1E, 0x1D, 0x1A, 0x1C, 0x1C, 0x20,
            0x24, 0x2E, 0x27, 0x20, 0x22, 0x2C, 0x23, 0x1C, 0x1C, 0x28, 0x37, 0x29,
            0x2C, 0x30, 0x31, 0x34, 0x34, 0x34, 0x1F, 0x27, 0x39, 0x3D, 0x38, 0x32,
            0x3C, 0x2E, 0x33, 0x34, 0x32, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x01,
            0x00, 0x01, 0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00,
            0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03,
            0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x12, 0x31, 0x41, 0x06,
            0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08,
            0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72,
            0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
            0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45,
            0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
            0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75,
            0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
            0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3,
            0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6,
            0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9,
            0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
            0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4,
            0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01,
            0x00, 0x00, 0x3F, 0x00, 0x7B, 0x94, 0x11, 0x00, 0x00, 0x00, 0x00, 0x49,
            0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let file_path = std::path::Path::new(&dir).join("test.jpg");
        tokio::fs::write(&file_path, &jpeg_bytes).await.unwrap();

        let result = tool
            .execute("c_jpg", serde_json::json!({"path": "test.jpg"}), None)
            .await
            .unwrap();

        match &result.content[0] {
            Content::Image { data, mime_type } => {
                assert_eq!(mime_type, "image/jpeg");
                use base64::Engine;
                let decoded = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
                assert_eq!(decoded.len(), jpeg_bytes.len(), "Should return full JPEG");
            }
            other => panic!("Expected Image content, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn read_text_file_is_not_image() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let file_path = std::path::Path::new(&dir).join("plain.txt");
        tokio::fs::write(&file_path, "hello world\n").await.unwrap();

        let result = tool
            .execute("c_txt", serde_json::json!({"path": "plain.txt"}), None)
            .await
            .unwrap();

        match &result.content[0] {
            Content::Text { text } => assert!(text.contains("hello world")),
            other => panic!("Expected Text content for .txt file, got {:?}", other),
        }
    }

    // ── macOS path tolerance ──────────────────────────────────────────────

    #[test]
    fn resolve_read_path_absolute() {
        let resolved = resolve_read_path("/tmp", "/some/dir");
        assert_eq!(resolved, "/tmp");
    }

    #[test]
    fn resolve_read_path_relative() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let resolved = resolve_read_path("Cargo.toml", &cwd);
        assert!(resolved.ends_with("Cargo.toml"), "resolved: {}", resolved);
    }

    #[test]
    fn try_nfd_variant_unicode() {
        // "é" in NFC (composed) is U+00E9; in NFD (decomposed) it's U+0065 + U+0301
        let nfc = "\u{00E9}"; // é in NFC
        let nfd = try_nfd_variant(nfc);
        assert_eq!(nfd, "\u{0065}\u{0301}", "NFD should decompose é to e + combining acute");
    }

    #[test]
    fn try_am_pm_variant_replaces_space() {
        let input = "screenshot 2024 AM.png";
        let result = try_am_pm_variant(input);
        assert!(result.contains('\u{202F}'), "Should contain NNBSP: {:?}", result);
        assert!(!result.contains(" AM."), "Should not contain original AM");
    }

    #[test]
    fn try_curly_quote_variant_replaces_apostrophe() {
        let input = "Capture d'ecran.png";
        let result = try_curly_quote_variant(input);
        assert!(result.contains('\u{2019}'), "Should contain right single quote: {:?}", result);
    }

    #[tokio::test]
    async fn resolve_read_path_finds_file_via_nfd() {
        // Write a file with an NFD name on disk
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        // Create a file with an NFC character "é" in the name
        let nfc_name = format!("test_{}.txt", '\u{00E9}'); // é in NFC
        let file_path = std::path::Path::new(&dir).join(&nfc_name);
        tokio::fs::write(&file_path, "nfc content\n").await.unwrap();

        // Read using existing tool that resolves via cwd
        let result = tool
            .execute("c_nfc", serde_json::json!({"path": &nfc_name}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("nfc content"), "Should find file with NFC name: got {}", text);
    }
}
