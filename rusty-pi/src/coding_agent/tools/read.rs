//! Read tool — read file contents.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/read.ts`.
//! Supports text files with offset/limit, and image files.

use crate::agent::types::{AgentTool, AgentToolResult};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::truncate::{format_size, truncate_head, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
        // Normalize the path
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

/// The read tool — reads file contents.
pub struct ReadTool {
    cwd: String,
}

impl ReadTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
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

        let absolute_path = resolve_to_cwd(&read_params.path, &self.cwd);
        let path = Path::new(&absolute_path);

        // Check if file exists
        if !path.exists() {
            anyhow::bail!("File not found: {}", read_params.path);
        }

        // Read file content
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", read_params.path, e))?;

        let all_lines: Vec<&str> = content.split('\n').collect();
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

    fn tool() -> ReadTool {
        ReadTool::new(
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        )
    }

    #[tokio::test]
    async fn read_existing_file() {
        // Read this source file itself
        let result = tool()
            .execute("c1", serde_json::json!({"path": "src/coding_agent/tools/read.rs"}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        // Should contain the tool name
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
        // Should start from line 1
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
        assert!(lines.len() <= 8, "Should have at most 5 content lines + continuation (got {})", lines.len()); // 5 lines + newlines + continuation hint
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

    #[test]
    fn resolve_path_absolute() {
        let cwd = "/some/dir";
        // On Unix, /tmp is absolute
        let resolved = resolve_to_cwd("/tmp", cwd);
        assert_eq!(resolved, "/tmp");
    }

    #[test]
    fn resolve_path_relative() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let resolved = resolve_to_cwd("Cargo.toml", &cwd);
        assert!(resolved.ends_with("Cargo.toml"), "resolved: {}", resolved);
    }
}
