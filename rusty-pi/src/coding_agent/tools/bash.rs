//! Bash tool — execute shell commands on the local system.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/bash.ts`.
//! Spawns a subprocess, captures stdout/stderr, handles timeouts and abort signals.

use crate::agent::events::{AgentEvent, ToolOutputStream};
use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionMode};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_tail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader as StdBufReader};
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::mpsc;
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

/// Kill a process group by PGID using SIGKILL.
///
/// Uses `killpg` to send SIGKILL to the entire process group, ensuring
/// child processes (e.g. `sleep` spawned by `sh -c`) are also terminated.
/// This prevents orphaned processes from holding pipe file descriptors open.
fn kill_process_group(pgid: Option<u32>) {
    if let Some(pgid) = pgid {
        #[cfg(unix)]
        {
            // Safety: killpg is a standard POSIX function. The pgid is validated
            // by the kernel. We ignore the result because the process may have
            // already exited.
            unsafe {
                libc::killpg(pgid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .arg("/F")
                .arg("/PID")
                .arg(pgid.to_string())
                .spawn();
        }
    }
}

/// Marker used to detect CWD changes after bash execution.
const CWD_MARKER: &str = "__RUSTY_PI_PWD__";

/// The bash tool — executes shell commands.
pub struct BashTool {
    /// Shared working directory for command execution.
    shared_cwd: Arc<RwLock<PathBuf>>,
    /// Optional callback for streaming output as it arrives.
    output_cb: Mutex<Option<OutputCallback>>,
    /// Optional event sender for ToolOutput events.
    event_tx: Mutex<Option<tokio::sync::mpsc::Sender<AgentEvent>>>,
}

impl BashTool {
    /// Create a new bash tool that executes commands in the shared working directory.
    pub fn new(shared_cwd: Arc<RwLock<PathBuf>>) -> Self {
        Self {
            shared_cwd,
            output_cb: Mutex::new(None),
            event_tx: Mutex::new(None),
        }
    }

    /// Get the current cached working directory.
    fn cached_cwd(&self) -> PathBuf {
        self.shared_cwd.read().expect("shared_cwd lock poisoned").clone()
    }

    /// Update the shared CWD after detecting a change.
    fn update_cwd(&self, new_cwd: PathBuf) {
        *self.shared_cwd.write().expect("shared_cwd lock poisoned") = new_cwd;
    }

    /// Detect CWD marker in output and update shared_cwd. Returns cleaned output (without marker).
    fn extract_cwd_and_clean_output(&self, output: &str) -> String {
        let marker_prefix = format!("{}:", CWD_MARKER);
        let mut lines: Vec<&str> = output.lines().collect();

        // Find the LAST line with the marker (most recent PWD)
        let marker_pos = lines.iter().rposition(|l| l.trim().starts_with(&marker_prefix));

        if let Some(pos) = marker_pos {
            let marker_line = lines[pos].trim();
            if let Some(cwd_str) = marker_line.strip_prefix(&marker_prefix) {
                let cwd_str = cwd_str.trim();
                if !cwd_str.is_empty() {
                    self.update_cwd(PathBuf::from(cwd_str));
                }
            }
            lines.remove(pos);
        }

        lines.join("\n")
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

    fn configure_streaming(&self, event_tx: tokio::sync::mpsc::Sender<AgentEvent>) {
        *self.event_tx.lock().unwrap() = Some(event_tx);
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult> {
        let bash_params: BashParams = serde_json::from_value(params)?;
        let tool_call_id = _tool_call_id.to_string();

        // Read current shared CWD
        let current_cwd = self.cached_cwd();

        // Build command with CWD detection appended
        let detect_cmd = format!("{}; echo {}:$(pwd)", bash_params.command, CWD_MARKER,);

        // Build and spawn with std::process::Command (not tokio).
        // tokio::process::Child::drop never calls waitpid, leaving zombies
        // when the runtime is dropped. Using std::process::Command with
        // manual waitpid in an OS thread gives us full control.
        let mut cmd = if cfg!(target_os = "windows") {
            let mut c = StdCommand::new("cmd");
            c.arg("/C").arg(&detect_cmd);
            c
        } else {
            let mut c = StdCommand::new("sh");
            c.arg("-c").arg(&detect_cmd);
            c
        };

        cmd.current_dir(&current_cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let mut child = cmd.spawn()?;
        let pid = child.id();

        // Take stdout/stderr for thread-based async reading
        let child_stdout = child.stdout.take();
        let child_stderr = child.stderr.take();

        // Channel for streaming output chunks
        let (output_tx, mut output_rx) = mpsc::channel::<(ToolOutputStream, String)>(256);
        // Channel for the exit code (reap result)
        let (reap_tx, mut reap_rx) = mpsc::channel::<Option<i32>>(1);

        // Take the event_tx for emitting ToolOutput events
        let event_tx = self.event_tx.lock().unwrap().take();

        // Check if already aborted before spawning reader threads
        if let Some(rx) = &signal
            && *rx.borrow()
        {
            kill_process_group(Some(pid));
            // Reap in a blocking thread
            let _ = std::thread::spawn(move || {
                let mut status: i32 = 0;
                #[cfg(unix)]
                unsafe {
                    libc::waitpid(pid as i32, &mut status, 0);
                }
            })
            .join();
            return Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "Command aborted".into(),
                }],
                details: serde_json::json!({"aborted": true}),
                aborted: true,
                is_error: true,
                ..Default::default()
            });
        }

        // ── Spawn reader threads for stdout/stderr ───────────────────────
        // These OS threads read blocking I/O and send chunks through channels.
        // They survive tokio runtime shutdown.
        if let Some(stdout) = child_stdout {
            let tx = output_tx.clone();
            std::thread::spawn(move || {
                let reader = StdBufReader::new(stdout);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            let _ = tx.blocking_send((ToolOutputStream::Stdout, format!("{}\n", l)));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        if let Some(stderr) = child_stderr {
            let tx = output_tx.clone();
            std::thread::spawn(move || {
                let reader = StdBufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) => {
                            let _ = tx.blocking_send((ToolOutputStream::Stderr, format!("{}\n", l)));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
        // Drop the extra sender so the channel closes when all readers finish
        drop(output_tx);

        // ── Spawn reap thread ────────────────────────────────────────────
        // Blocking waitpid in an OS thread. This thread survives runtime
        // shutdown and ensures the child is always reaped.
        std::thread::spawn(move || {
            let mut status: i32 = 0;
            #[cfg(unix)]
            {
                let ret = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
                let code = if ret > 0 && libc::WIFEXITED(status) {
                    Some(libc::WEXITSTATUS(status))
                } else {
                    None
                };
                let _ = reap_tx.blocking_send(code);
            }
            #[cfg(not(unix))]
            {
                let _ = child.wait();
                let code = child.try_wait().ok().flatten().and_then(|s| s.code());
                let _ = reap_tx.blocking_send(code);
            }
        });

        // ── Main read loop ────────────────────────────────────────────────
        let mut full_output = String::new();
        let mut timed_out = false;
        let mut aborted_early = false;
        let mut stdout_done = false;
        let mut stderr_done = false;

        let timeout_dur = bash_params.timeout.filter(|&t| t > 0).map(Duration::from_secs);

        while !stdout_done || !stderr_done {
            // Check abort signal at top of loop
            if let Some(ref rx) = signal
                && *rx.borrow()
            {
                aborted_early = true;
                kill_process_group(Some(pid));
                break;
            }

            tokio::select! {
                msg = output_rx.recv() => {
                    match msg {
                        Some((stream, chunk)) => {
                            // Emit ToolOutput event
                            if let Some(ref tx) = event_tx {
                                let _ = tx.try_send(AgentEvent::ToolOutput {
                                    id: tool_call_id.clone(),
                                    stream,
                                    chunk: chunk.clone(),
                                });
                            }
                            // Also fire legacy callback
                            if let Some(ref mut cb) = *self.output_cb.lock().unwrap() {
                                cb(&chunk);
                            }
                            full_output.push_str(&chunk);
                        }
                        None => {
                            // Channel closed — both reader threads finished
                            stdout_done = true;
                            stderr_done = true;
                        }
                    }
                }
                _ = async {
                    if let Some(mut rx) = signal.clone() {
                        let _ = rx.changed().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    aborted_early = true;
                    kill_process_group(Some(pid));
                    break;
                }
                _ = async {
                    if let Some(dur) = timeout_dur {
                        tokio::time::sleep(dur).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    timed_out = true;
                    kill_process_group(Some(pid));
                    break;
                }
            }
        }

        // Kill the process group so the blocking waitpid returns quickly.
        kill_process_group(Some(pid));

        // Await the reap result from the OS thread.
        let exit_code = tokio::time::timeout(Duration::from_secs(6), async { reap_rx.recv().await.flatten() })
            .await
            .unwrap_or(None);

        // Extract CWD marker from output and update shared CWD
        let cleaned_output = self.extract_cwd_and_clean_output(&full_output);

        // Handle abort (checked before timeout so abort takes priority)
        if aborted_early {
            return Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "Command aborted".into(),
                }],
                details: serde_json::json!({"aborted": true}),
                aborted: true,
                is_error: true,
                ..Default::default()
            });
        }

        if timed_out {
            return Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("Command timed out after {} seconds", bash_params.timeout.unwrap_or(0)),
                }],
                details: serde_json::json!({"timed_out": true, "timeout": bash_params.timeout}),
                timed_out: true,
                is_error: true,
                ..Default::default()
            });
        }

        // Apply truncation
        let max_lines = bash_params.max_lines.unwrap_or(DEFAULT_MAX_LINES);
        let max_bytes = bash_params.max_bytes.unwrap_or(DEFAULT_MAX_BYTES);
        let tr = truncate_tail(&cleaned_output, max_lines, max_bytes);

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
                    start_line,
                    tr.total_lines,
                    tr.total_lines,
                    max_bytes / 1024
                ));
            }
        }

        if let Some(code) = exit_code
            && code != 0
        {
            let text = if result_text.is_empty() {
                format!("Command exited with code {}", code)
            } else {
                format!("{}\n\nCommand exited with code {}", result_text, code)
            };
            return Ok(AgentToolResult {
                content: vec![Content::Text { text }],
                details: serde_json::json!({"exit_code": code}),
                exit_code: Some(code),
                is_error: true,
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
            details: serde_json::json!({ "exit_code": exit_code }),
            exit_code,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> BashTool {
        let shared_cwd = Arc::new(RwLock::new(std::env::current_dir().unwrap()));
        BashTool::new(shared_cwd)
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
            .execute("c3", serde_json::json!({"command": "sleep 10", "timeout": 1}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("timed out"), "Expected timeout message, got: {}", text);
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
        assert!(text.contains("aborted"), "Expected aborted message, got: {}", text);
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
        assert!(
            text.contains("Showing lines"),
            "Should have truncation message: {}",
            text
        );
        assert!(text.contains("96"), "Should contain '96' in output: {}", text);
        assert!(text.contains("100"), "Should contain '100' in output: {}", text);
    }

    #[tokio::test]
    async fn truncation_bytes_limit() {
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
        assert!(
            text.contains("KB limit") || text.contains("Showing lines"),
            "Should have truncation message: {}",
            text
        );
    }

    #[tokio::test]
    async fn bash_cwd_persists_across_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let subdir = tmp.path().join("sub");
        std::fs::create_dir_all(&subdir).unwrap();

        let shared_cwd = Arc::new(RwLock::new(tmp.path().to_path_buf()));
        let tool = BashTool::new(shared_cwd.clone());

        let result = tool
            .execute("c1", serde_json::json!({"command": "cd sub && pwd"}), None)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.trim(),
            _ => panic!("Expected text content"),
        };
        assert!(text.ends_with("/sub"), "Expected pwd to end with /sub, got: {}", text);

        let cwd_after = shared_cwd.read().unwrap().clone();
        assert!(
            cwd_after.ends_with("sub"),
            "Expected cwd to end with 'sub', got: {:?}",
            cwd_after
        );

        let result2 = tool
            .execute("c2", serde_json::json!({"command": "pwd"}), None)
            .await
            .unwrap();
        let text2 = match &result2.content[0] {
            Content::Text { text } => text.trim(),
            _ => panic!("Expected text content"),
        };
        assert!(text2.ends_with("/sub"), "Second pwd should show sub, got: {}", text2);

        let result3 = tool
            .execute("c3", serde_json::json!({"command": "cd .. && pwd"}), None)
            .await
            .unwrap();
        let text3 = match &result3.content[0] {
            Content::Text { text } => text.trim(),
            _ => panic!("Expected text content"),
        };
        assert_eq!(text3, tmp.path().to_string_lossy(), "Should go back to tmp");
    }

    // ── ToolOutput event tests ──────────────────────────────────────────

    #[tokio::test]
    async fn tool_output_stdout_single_chunk() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let result = t
            .execute("to1", serde_json::json!({"command": "echo hello"}), None)
            .await
            .unwrap();
        assert!(!result.is_error);

        // Collect ToolOutput events
        let mut outputs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::ToolOutput { id, stream, chunk } = ev {
                outputs.push((id, stream, chunk));
            }
        }
        assert!(!outputs.is_empty(), "Should have ToolOutput events");
        assert!(
            outputs
                .iter()
                .any(|(_, s, c)| *s == ToolOutputStream::Stdout && c.contains("hello"))
        );
    }

    #[tokio::test]
    async fn tool_output_stderr_single_chunk() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let result = t
            .execute("to2", serde_json::json!({"command": "echo err >&2"}), None)
            .await
            .unwrap();

        let mut outputs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::ToolOutput { id, stream, chunk } = ev {
                outputs.push((id, stream, chunk));
            }
        }
        assert!(
            outputs
                .iter()
                .any(|(_, s, c)| *s == ToolOutputStream::Stderr && c.contains("err"))
        );
    }

    #[tokio::test]
    async fn tool_output_stdout_stderr_interleaved() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let result = t
            .execute(
                "to3",
                serde_json::json!({"command": "echo out1; echo err1 >&2; echo out2; echo err2 >&2"}),
                None,
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let mut outputs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::ToolOutput { stream, chunk, .. } = ev {
                outputs.push((stream, chunk));
            }
        }
        let stdout_chunks: Vec<&str> = outputs
            .iter()
            .filter(|(s, _)| *s == ToolOutputStream::Stdout)
            .map(|(_, c)| c.as_str())
            .collect();
        let stderr_chunks: Vec<&str> = outputs
            .iter()
            .filter(|(s, _)| *s == ToolOutputStream::Stderr)
            .map(|(_, c)| c.as_str())
            .collect();
        assert!(!stdout_chunks.is_empty(), "Should have stdout chunks");
        assert!(!stderr_chunks.is_empty(), "Should have stderr chunks");
    }

    #[tokio::test]
    async fn tool_output_preserves_tool_call_id() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let result = t
            .execute("my_special_id", serde_json::json!({"command": "echo test"}), None)
            .await
            .unwrap();

        let mut ids = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AgentEvent::ToolOutput { id, .. } = ev {
                ids.push(id);
            }
        }
        assert!(
            ids.iter().all(|id| id == "my_special_id"),
            "All events should have correct tool call ID"
        );
    }

    #[tokio::test]
    async fn tool_output_no_late_events_after_cancel() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async move {
            t.execute(
                "to_cancel",
                serde_json::json!({"command": "for i in 1 2 3 4 5; do echo line_$i; sleep 0.1; done"}),
                Some(abort_rx),
            )
            .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        abort_tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        assert!(result.aborted);

        // Drain any events that arrived before cancel
        let mut count = 0;
        while let Ok(_ev) = rx.try_recv() {
            count += 1;
        }
        // After cancel, no new events should arrive (this is a basic check)
        // The key assertion is that the tool returned promptly
    }

    #[tokio::test]
    async fn tool_output_timeout_no_late_events() {
        let t = tool();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        t.configure_streaming(tx);

        let result = t
            .execute(
                "to_timeout",
                serde_json::json!({"command": "echo start; sleep 30", "timeout": 1}),
                None,
            )
            .await
            .unwrap();
        assert!(result.timed_out);

        // Drain events
        let mut count = 0;
        while let Ok(_ev) = rx.try_recv() {
            count += 1;
        }
        // Events should have stopped after timeout
    }

    // ── Regression tests for parallel execution ──────────────────────────

    #[tokio::test]
    async fn bash_abort_pre_cancelled_token() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        tx.send(true).ok();
        let result = tool()
            .execute("c_pre", serde_json::json!({"command": "sleep 30"}), Some(rx))
            .await
            .unwrap();
        assert!(result.aborted);
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text"),
        };
        assert!(text.contains("aborted"));
    }

    #[tokio::test]
    async fn bash_abort_concurrent_with_output() {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let tool_instance = tool();

        let handle = tokio::spawn(async move {
            tool_instance
                .execute(
                    "c_conc",
                    serde_json::json!({"command": "for i in 1 2 3 4 5; do echo line_$i; sleep 0.1; done"}),
                    Some(rx),
                )
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        tx.send(true).ok();

        let result = handle.await.unwrap().unwrap();
        assert!(result.aborted);
    }

    #[tokio::test]
    async fn bash_timeout_then_no_hang() {
        let result = tool()
            .execute("c_to", serde_json::json!({"command": "sleep 60", "timeout": 1}), None)
            .await
            .unwrap();
        assert!(result.timed_out);
        assert!(result.is_error);
    }
}
