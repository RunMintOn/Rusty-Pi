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
use tokio::process::Command;
use tokio::time::Duration;

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
}

impl BashTool {
    /// Create a new bash tool that executes commands in `cwd`.
    pub fn new(cwd: String) -> Self {
        Self { cwd }
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

        // Build a future for waiting on the abort signal
        let abort_future = async move {
            if let Some(mut rx) = signal {
                let _ = rx.changed().await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        // Wait with timeout and abort support
        let timeout_dur = bash_params
            .timeout
            .filter(|&t| t > 0)
            .map(Duration::from_secs);

        let output = if let Some(dur) = timeout_dur {
            // With timeout
            tokio::select! {
                result = child.wait_with_output() => {
                    match result {
                        Ok(out) => out,
                        Err(e) => return Err(e.into()),
                    }
                }
                _ = abort_future => {
                    kill_process(pid);
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Command aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }
                _ = tokio::time::sleep(dur) => {
                    kill_process(pid);
                    return Ok(AgentToolResult {
                        content: vec![Content::Text {
                            text: format!("Command timed out after {} seconds", bash_params.timeout.unwrap()),
                        }],
                        details: serde_json::json!({"timed_out": true, "timeout": bash_params.timeout}),
                        ..Default::default()
                    });
                }
            }
        } else {
            // Without timeout
            tokio::select! {
                result = child.wait_with_output() => {
                    match result {
                        Ok(out) => out,
                        Err(e) => return Err(e.into()),
                    }
                }
                _ = abort_future => {
                    kill_process(pid);
                    return Ok(AgentToolResult {
                        content: vec![Content::Text { text: "Command aborted".into() }],
                        details: serde_json::json!({"aborted": true}),
                        ..Default::default()
                    });
                }
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut full_output = String::new();
        if !stdout.is_empty() {
            full_output.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !full_output.is_empty() {
                full_output.push('\n');
            }
            full_output.push_str(&stderr);
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

        let exit_code = output.status.code();

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

}
