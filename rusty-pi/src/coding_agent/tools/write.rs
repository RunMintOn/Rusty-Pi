//! Write tool — write content to files.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/write.ts`.
//! Creates parent directories, handles abort signals, and serializes writes
//! to the same file path via a mutation queue.

use crate::agent::types::{AgentTool, AgentToolResult};
use crate::ai::types::{Content, Tool};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// Parameters for the write tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WriteParams {
    /// Path to the file to write (relative or absolute).
    pub path: String,
    /// Content to write to the file.
    pub content: String,
}

/// Resolve a path relative to the current working directory.
fn resolve_to_cwd(file_path: &str, cwd: &str) -> String {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_string_lossy().to_string()
    } else {
        Path::new(cwd).join(path).to_string_lossy().to_string()
    }
}

/// Global per-path mutexes to serialize writes to the same file path.
///
/// This ensures that concurrent writes to the same file are queued and executed
/// one at a time, avoiding race conditions.
static MUTATION_LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Execute a file mutation, serializing operations to the same file path.
pub async fn with_file_mutation_queue<F, Fut, T>(absolute_path: &str, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let lock = {
        let mut map = MUTATION_LOCKS.lock().unwrap();
        map.entry(absolute_path.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };

    let _guard = lock.lock().await;
    f().await
}

/// The write tool — writes content to files.
pub struct WriteTool {
    shared_cwd: Arc<RwLock<PathBuf>>,
}

impl WriteTool {
    pub fn new(shared_cwd: Arc<RwLock<PathBuf>>) -> Self {
        Self { shared_cwd }
    }

    fn cwd(&self) -> String {
        self.shared_cwd.read().unwrap().to_string_lossy().to_string()
    }
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. \
Automatically creates parent directories."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative or absolute)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }
}

#[async_trait]
impl AgentTool for WriteTool {
    fn label(&self) -> &str {
        "write"
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult> {
        let write_params: WriteParams = serde_json::from_value(params)
            .map_err(|e| anyhow::anyhow!("Invalid write parameters: {}", e))?;

        let absolute_path = resolve_to_cwd(&write_params.path, &self.cwd());
        let path = Path::new(&absolute_path);

        // Use the mutation queue to serialize writes to this file
        with_file_mutation_queue(&absolute_path, || async {
            // Check abort
            if let Some(rx) = &signal
                && *rx.borrow() {
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Operation aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }

            // Create parent directories
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await
                    .map_err(|e| anyhow::anyhow!("Failed to create parent directories for '{}': {}", write_params.path, e))?;
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

            // Write the file
            tokio::fs::write(path, &write_params.content).await
                .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", write_params.path, e))?;

            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("Successfully wrote {} bytes to {}", write_params.content.len(), write_params.path),
                }],
                details: serde_json::json!({"bytes_written": write_params.content.len()}),
                ..Default::default()
            })
        }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    /// Create a WriteTool with a temp directory as cwd.
    async fn tool_and_temp() -> (WriteTool, Arc<TokioMutex<tempfile::TempDir>>) {
        let tmp = tempfile::tempdir().unwrap();
        let shared_cwd = Arc::new(RwLock::new(tmp.path().to_path_buf()));
        let tool = WriteTool::new(shared_cwd);
        let tmp_arc = Arc::new(TokioMutex::new(tmp));
        (tool, tmp_arc)
    }

    #[tokio::test]
    async fn write_new_file() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let result = tool
            .execute("c1", serde_json::json!({"path": "test.txt", "content": "hello world"}), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully wrote"), "Got: {}", text);
        assert!(text.contains("11 bytes"), "Got: {}", text);

        // Verify file content
        let content = tokio::fs::read_to_string(Path::new(&dir).join("test.txt")).await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn write_overwrite_existing() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();
        let file_path = Path::new(&dir).join("existing.txt");

        // Create existing file
        tokio::fs::write(&file_path, "old content").await.unwrap();

        // Overwrite
        let result = tool
            .execute("c2", serde_json::json!({"path": "existing.txt", "content": "new content"}), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully wrote"));

        // Verify overwritten
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn write_creates_parent_dirs() {
        let (tool, tmp) = tool_and_temp().await;
        let dir = tmp.lock().await.path().to_string_lossy().to_string();

        let result = tool
            .execute("c3", serde_json::json!({"path": "sub/dir/nested/file.txt", "content": "nested"}), None)
            .await
            .unwrap();

        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Successfully wrote"), "Got: {:?}", result.content);

        // Verify file exists
        let content = tokio::fs::read_to_string(Path::new(&dir).join("sub/dir/nested/file.txt")).await.unwrap();
        assert_eq!(content, "nested");
    }

    #[tokio::test]
    async fn write_abort() {
        let (tool, tmp) = tool_and_temp().await;
        let _dir = tmp.lock().await.path().to_string_lossy().to_string();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let tool_instance = tool;

        let handle = tokio::spawn(async move {
            tool_instance
                .execute("c4", serde_json::json!({"path": "aborted.txt", "content": "should not appear"}), Some(rx))
                .await
        });

        tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.as_str(),
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("aborted"), "Expected aborted, got: {}", text);
    }
}
