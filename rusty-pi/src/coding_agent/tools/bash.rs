//! Bash tool — execute shell commands on the local system.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/bash.ts`.
//! Spawns a subprocess, captures stdout/stderr, handles timeouts and abort signals.

use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionMode};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::truncate::{truncate_tail, DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::time::Duration;

/// Callback for streaming bash output chunks as they arrive.
type OutputCallback = Box<dyn FnMut(&str) + Send>;

/// Parameters for the bash tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashParams {
    /// Bash command to execute.
    pub command: String,
    /// Timeout in seconds (optional, no default timeout).
    pub timeout: Option<u64>,
    /// Maximum output lines before truncation (default: 2000).
    pub max_lines: Option<usize>,
    /// Maximum output bytes before truncation (default: 50 KB).
    pub max_bytes: Option<usize>,
}

/// Kill a process by PID using the OS native command.
fn kill_process(pid: Option<u32>) {
    if let Some(pid) = pid {
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill")
                .arg("-9")
                .arg(pid.to_string())
                .spawn();
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .arg("/F")
                .arg("/PID")
                .arg(pid.to_string())
                .spawn();
        }
    }
}

/// The bash tool — executes shell commands.
pub struct BashTool {
    /// Working directory for command execution.
    cwd: String,
    /// Optional callback for streaming output as it arrives.
    output_cb: Mutex<Option<OutputCallback>>,
}

impl BashTool {
    /// Create a new bash tool that executes commands in `cwd`.
    pub fn new(cwd: String) -> Self {
        Self { cwd, output_cb: Mutex::new(None) }
    }

    /// Register a callback invoked for each chunk of stdout/stderr as it arrives.
    /// The callback receives the raw text (may include partial lines).
    pub fn on_output<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.output_cb.lock().unwrap().replace(Box::new(callback));
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the current working directory. Returns stdout and stderr. \
Truncates output to last 2000 lines or 50 KB (configurable via max_lines/max_bytes). \
Optionally provide a timeout in seconds."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute"
                },
                "timeout": {
                    "type": "number",
                    "description": "Timeout in seconds (optional, no default timeout)"
                },
                "max_lines": {
                    "type": "number",
                    "description": "Maximum output lines (optional, default: 2000)"
                },
                "max_bytes": {
                    "type": "number",
                    "description": "Maximum output bytes (optional, default: 51200 = 50 KB)"
                }
            },
            "required": ["command"]
        })
    }
}

#[async_trait]
impl AgentTool for BashTool {
    fn label(&self) -> &str {
        "bash"
    }

    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult> {
        let bash_params: BashParams = serde_json::from_value(params)?;

        // Build and spawn the command
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&bash_params.command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(&bash_params.command);
            c
        };

        cmd.current_dir(&self.cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let pid = child.id();

        // Check if already aborted
        if let Some(rx) = &signal
            && *rx.borrow() {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Ok(AgentToolResult {
                    content: vec![Content::Text { text: "Command aborted".into() }],
                    details: serde_json::json!({"aborted": true}),
                    ..Default::default()
                });
            }

        // Take stdout/stderr pipes for streaming reads
        let stdout = child.stdout.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stdout"))?;
        let stderr = child.stderr.take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture stderr"))?;

        let mut stdout_reader = tokio::io::BufReader::new(stdout);
        let mut stderr_reader = tokio::io::BufReader::new(stderr);
        let mut full_output = String::new();

        // Read lines from both pipes concurrently, streaming via callback
        let mut stdout_line = String::new();
        let mut stderr_line = String::new();
        let mut stdout_done = false;
        let mut stderr_done = false;
        let mut timed_out = false;

        let timeout_dur = bash_params
            .timeout
            .filter(|&t| t > 0)
            .map(Duration::from_secs);

        while !stdout_done || !stderr_done {
            // Check abort signal
            if let Some(ref rx) = signal
                && *rx.borrow() {
                    kill_process(pid);
                    let _ = child.wait().await;
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Command aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }

            tokio::select! {
                result = async {
                    if stdout_done { return Ok(0usize); }
                    stdout_reader.read_line(&mut stdout_line).await
                } => {
                    let n = result?;
                    if n == 0 {
                        stdout_done = true;
                    } else {
                        if let Some(ref mut cb) = *self.output_cb.lock().unwrap() {
                            cb(&stdout_line);
                        }
                        full_output.push_str(&stdout_line);
                        stdout_line.clear();
                    }
                }
                result = async {
                    if stderr_done { return Ok(0usize); }
                    stderr_reader.read_line(&mut stderr_line).await
                } => {
                    let n = result?;
                    if n == 0 {
                        stderr_done = true;
                    } else {
                        if let Some(ref mut cb) = *self.output_cb.lock().unwrap() {
                            cb(&stderr_line);
                        }
                        full_output.push_str(&stderr_line);
                        stderr_line.clear();
                    }
                }
                _ = async {
                    if let Some(mut rx) = signal.clone() {
                        let _ = rx.changed().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    kill_process(pid);
                    let _ = child.wait().await;
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Command aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }
                _ = async {
                    if let Some(dur) = timeout_dur {
                        tokio::time::sleep(dur).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    timed_out = true;
                    kill_process(pid);
                    let _ = child.wait().await;
                }
            }
        }

        // Wait for process to fully exit
        let exit_status = child.wait().await?;
        let exit_code = exit_status.code();

        if timed_out {
            return Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("Command timed out after {} seconds", bash_params.timeout.unwrap_or(0)),
                }],
                details: serde_json::json!({"timed_out": true, "timeout": bash_params.timeout}),
                ..Default::default()
            });
        }

        // Apply truncation
        let max_lines = bash_params.max_lines.unwrap_or(DEFAULT_MAX_LINES);
        let max_bytes = bash_params.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        let tr = truncate_tail(&full_output, max_lines, max_bytes);

        let mut result_text = tr.content.trim().to_string();
        if tr.truncated {
            let start_line = if tr.output_lines < tr.total_lines {
                tr.total_lines - tr.output_lines + 1
            } else {
                1
            };
            if tr.truncated_by == "lines" {
                result_text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}]\n",
                    start_line, tr.total_lines, tr.total_lines
                ));
            } else {
                result_text.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {} ({} KB limit)]\n",
                    start_line, tr.total_lines, tr.total_lines, max_bytes / 1024
                ));
            }
        }

        if let Some(code) = exit_code
            && code != 0 {
                let text = if result_text.is_empty() {
                    format!("Command exited with code {}", code)
                } else {
                    format!("{}\n\nCommand exited with code {}", result_text, code)
                };
                return Ok(AgentToolResult {
                    content: vec![Content::Text { text }],
                    details: serde_json::json!({"exit_code": code}),
                    ..Default::default()
                });
            }

        let final_text = if result_text.is_empty() {
            "(no output)".into()
        } else {
            result_text
        };

        Ok(AgentToolResult {
            content: vec![Content::Text { text: final_text }],
            details: serde_json::json!({"exit_code": exit_code}),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> BashTool {
        BashTool::new(
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        )
    }

    #[tokio::test]
    async fn bash_echo() {
        let result = tool()
            .execute("c1", serde_json::json!({"command": "echo hello world"}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.trim().to_string(),
            _ => panic!("Expected text content"),
        };
        assert_eq!(text, "hello world");
    }

    #[tokio::test]
    async fn bash_failing_command() {
        let result = tool()
            .execute("c2", serde_json::json!({"command": "exit 42"}), None)
            .await
            .unwrap();
        assert_eq!(result.details["exit_code"], 42);
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("exited with code 42"));
    }

    #[tokio::test]
    async fn bash_timeout() {
        let result = tool()
            .execute(
                "c3",
                serde_json::json!({"command": "sleep 10", "timeout": 1}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(
            text.contains("timed out"),
            "Expected timeout message, got: {}",
            text
        );
    }

    #[tokio::test]
    async fn bash_abort() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let tool_instance = tool();

        let handle = tokio::spawn(async move {
            tool_instance
                .execute("c4", serde_json::json!({"command": "sleep 30"}), Some(rx))
                .await
        });

        tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(
            text.contains("aborted"),
            "Expected aborted message, got: {}",
            text
        );
    }

    #[tokio::test]
    async fn bash_with_output_and_timeout() {
        let result = tool()
            .execute(
                "c5",
                serde_json::json!({"command": "echo hi && sleep 0.5 && echo bye", "timeout": 10}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("hi"));
        assert!(text.contains("bye"));
    }

    #[tokio::test]
    async fn truncation_under_limit_returns_full() {
        let result = tool()
            .execute(
                "c6",
                serde_json::json!({"command": "echo small", "max_lines": 100, "max_bytes": 99999}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.trim(),
            _ => panic!("Expected text content"),
        };
        assert_eq!(text, "small");
        assert!(!text.contains("Showing lines"), "Should not have truncation message");
    }

    #[tokio::test]
    async fn truncation_lines_limit() {
        // Generate output with many lines using seq, then truncate with max_lines=5
        let result = tool()
            .execute(
                "c7",
                serde_json::json!({"command": "seq 1 100", "max_lines": 5, "max_bytes": 99999}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("Showing lines"), "Should have truncation message: {}", text);
        // Last few lines (96-100) should be present
        assert!(text.contains("96"), "Should contain '96' in output: {}", text);
        assert!(text.contains("100"), "Should contain '100' in output: {}", text);
    }

    #[tokio::test]
    async fn truncation_bytes_limit() {
        // Generate a line that exceeds the byte limit
        let result = tool()
            .execute(
                "c8",
                serde_json::json!({"command": "echo short && echo long_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "max_lines": 9999, "max_bytes": 30}),
                None,
            )
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("KB limit") || text.contains("Showing lines"), "Should have truncation message: {}", text);
        // First line "short" is part of the last lines kept, so it should be there or truncated
    }

    #[tokio::test]
    async fn bash_cwd_not_found() {
        let tool = BashTool::new("/nonexistent/path/that/does/not/exist".into());
        let result = tool
            .execute("c9", serde_json::json!({"command": "echo hi"}), None)
            .await;
        assert!(result.is_err(), "Should error when cwd does not exist");
        let err = result.unwrap_err().to_string();
        // The error should mention the directory or no such file
        assert!(
            err.contains("No such file") || err.contains("nicht gefunden") || err.contains("nonexistent") || err.contains("path"),
            "Expected error mentioning directory issue, got: {}",
            err
        );
    }
}
