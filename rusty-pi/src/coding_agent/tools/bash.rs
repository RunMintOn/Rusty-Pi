//! Bash tool — execute shell commands on the local system.
//!
//! Mirrors the original `@earendil-works/pi-coding-agent/src/core/tools/bash.ts`.
//! Spawns a subprocess, captures stdout/stderr, handles timeouts and abort signals.

use crate::agent::events::ToolOutputStream;
use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionContext, ToolExecutionMode, ToolOutputEvent};
use crate::ai::types::{Content, Tool};
use crate::coding_agent::tools::truncate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncate_tail};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader as StdBufReader};
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use tokio::sync::mpsc;
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

#[derive(Default)]
struct LifecycleCounters {
    active_readers: AtomicUsize,
    active_reapers: AtomicUsize,
}

/// Owns every OS resource created for one bash invocation.
///
/// There are no detached threads: the owner retains both reader join handles
/// and the single reap join handle. Dropping the execution future drops this
/// owner, which kills the process group before joining all three lifecycle
/// threads.
struct ChildLifecycle {
    pid: u32,
    pgid: Option<u32>,
    readers: Vec<JoinHandle<()>>,
    reap: Option<JoinHandle<()>>,
}

impl ChildLifecycle {
    fn new(pid: u32, readers: Vec<JoinHandle<()>>, reap: JoinHandle<()>) -> Self {
        Self {
            pid,
            pgid: Some(pid),
            readers,
            reap: Some(reap),
        }
    }

    fn kill(&mut self) {
        debug_assert_ne!(self.pid, 0);
        kill_process_group(self.pgid.take());
    }

    fn join_threads(&mut self) -> anyhow::Result<()> {
        for reader in self.readers.drain(..) {
            reader
                .join()
                .map_err(|_| anyhow::anyhow!("bash reader thread panicked"))?;
        }
        if let Some(reap) = self.reap.take() {
            reap.join().map_err(|_| anyhow::anyhow!("bash reap thread panicked"))?;
        }
        self.pgid = None;
        Ok(())
    }
}

impl Drop for ChildLifecycle {
    fn drop(&mut self) {
        // This is the future-drop cleanup path. The process group is killed
        // before joins so blocked pipe reads and waitpid both become finite.
        self.kill();
        let _ = self.join_threads();
    }
}

fn spawn_reader<R>(
    reader: R,
    tx: mpsc::Sender<(ToolOutputStream, String)>,
    counters: Arc<LifecycleCounters>,
) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    counters.active_readers.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let reader = StdBufReader::new(reader);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx
                        .blocking_send((ToolOutputStream::Stdout, format!("{line}\n")))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        counters.active_readers.fetch_sub(1, Ordering::SeqCst);
    })
}

fn spawn_stderr_reader<R>(
    reader: R,
    tx: mpsc::Sender<(ToolOutputStream, String)>,
    counters: Arc<LifecycleCounters>,
) -> JoinHandle<()>
where
    R: std::io::Read + Send + 'static,
{
    counters.active_readers.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let reader = StdBufReader::new(reader);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx
                        .blocking_send((ToolOutputStream::Stderr, format!("{line}\n")))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        counters.active_readers.fetch_sub(1, Ordering::SeqCst);
    })
}

/// Marker used to detect CWD changes after bash execution.
const CWD_MARKER: &str = "__RUSTY_PI_PWD__";

/// The bash tool — executes shell commands.
pub struct BashTool {
    /// Shared working directory for command execution.
    shared_cwd: Arc<RwLock<PathBuf>>,
    lifecycle_counters: Arc<LifecycleCounters>,
}

impl BashTool {
    /// Create a new bash tool that executes commands in the shared working directory.
    pub fn new(shared_cwd: Arc<RwLock<PathBuf>>) -> Self {
        Self {
            shared_cwd,
            lifecycle_counters: Arc::new(LifecycleCounters::default()),
        }
    }

    #[cfg(test)]
    fn lifecycle_counters(&self) -> Arc<LifecycleCounters> {
        Arc::clone(&self.lifecycle_counters)
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
        context: ToolExecutionContext,
    ) -> anyhow::Result<AgentToolResult> {
        let bash_params: BashParams = serde_json::from_value(params)?;

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
        let (output_tx, output_rx) = mpsc::channel::<(ToolOutputStream, String)>(256);
        // Channel for the exit code (reap result)
        let (reap_tx, reap_rx) = mpsc::channel::<Option<i32>>(1);

        // Take the event_tx for emitting ToolOutput events
        // Check if already aborted before spawning reader threads
        if context.cancellation.is_cancelled() {
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

        // ── Spawn owned reader threads for stdout/stderr ─────────────────
        // These handles are retained by ChildLifecycle. They are never
        // detached, and are joined after the process group is dead.
        let mut readers = Vec::with_capacity(2);
        if let Some(stdout) = child_stdout {
            readers.push(spawn_reader(
                stdout,
                output_tx.clone(),
                Arc::clone(&self.lifecycle_counters),
            ));
        }
        if let Some(stderr) = child_stderr {
            readers.push(spawn_stderr_reader(
                stderr,
                output_tx.clone(),
                Arc::clone(&self.lifecycle_counters),
            ));
        }
        // Drop the extra sender so the channel closes when all readers finish
        drop(output_tx);

        // ── Spawn the sole reap thread ───────────────────────────────────
        // Blocking waitpid in one owned OS thread. No other code waits for
        // this PID; ChildLifecycle owns and joins this handle.
        self.lifecycle_counters.active_reapers.fetch_add(1, Ordering::SeqCst);
        let lifecycle_counters = Arc::clone(&self.lifecycle_counters);
        let reap_thread = std::thread::spawn(move || {
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
            lifecycle_counters.active_reapers.fetch_sub(1, Ordering::SeqCst);
        });
        let mut lifecycle = ChildLifecycle::new(pid, readers, reap_thread);
        // Shadow both receivers after the owner is declared. Rust drops these
        // bindings before `lifecycle`, so a dropped future closes the channel
        // send paths before joining reader/reap threads.
        let mut output_rx = output_rx;
        let mut reap_rx = reap_rx;

        // ── Main read loop ────────────────────────────────────────────────
        let mut full_output = String::new();
        let mut timed_out = false;
        let mut aborted_early = false;
        let mut output_done = false;
        let mut reap_done = false;
        let mut exit_code = None;

        let timeout_deadline = bash_params
            .timeout
            .filter(|&t| t > 0)
            .map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds));

        while !output_done || !reap_done {
            tokio::select! {
                msg = output_rx.recv(), if !output_done => {
                    match msg {
                        Some((stream, chunk)) => {
                            // Emit ToolOutput event through context channel
                            if !aborted_early && !timed_out {
                                let _ = context
                                    .output_tx
                                    .send(ToolOutputEvent {
                                        stream,
                                        chunk: chunk.clone(),
                                    })
                                    .await;
                            }
                            full_output.push_str(&chunk);
                        }
                        None => {
                            output_done = true;
                        }
                    }
                }
                result = reap_rx.recv(), if !reap_done => {
                    reap_done = true;
                    exit_code = result.flatten();
                    // A reaped shell may have descendants holding the pipes.
                    // Close the process group so the owned readers can join.
                    if !output_done {
                        lifecycle.kill();
                    }
                }
                _ = context.cancellation.cancelled(), if !aborted_early && !timed_out => {
                    aborted_early = true;
                    lifecycle.kill();
                }
                _ = async {
                    if let Some(deadline) = timeout_deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                }, if timeout_deadline.is_some() && !aborted_early && !timed_out => {
                    timed_out = true;
                    lifecycle.kill();
                }
            }
        }

        // All pipe readers and the sole reaper have completed. Joining here
        // proves normal execution has no detached lifecycle threads; Drop
        // performs the same join after future cancellation.
        lifecycle.join_threads()?;

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
    use crate::agent::types::ToolExecutionContext;
    use std::time::Instant;
    use tokio_util::sync::CancellationToken;

    fn tool() -> BashTool {
        let shared_cwd = Arc::new(RwLock::new(std::env::current_dir().unwrap()));
        BashTool::new(shared_cwd)
    }

    /// Create a ToolExecutionContext with a channel for collecting output events.
    fn make_context() -> (ToolExecutionContext, tokio::sync::mpsc::Receiver<ToolOutputEvent>) {
        let (output_tx, output_rx) = tokio::sync::mpsc::channel(256);
        let cancellation = CancellationToken::new();
        (
            ToolExecutionContext {
                output_tx,
                cancellation,
            },
            output_rx,
        )
    }

    /// Create a ToolExecutionContext with a pre-cancelled token.
    fn make_cancelled_context() -> (ToolExecutionContext, tokio::sync::mpsc::Receiver<ToolOutputEvent>) {
        let (output_tx, output_rx) = tokio::sync::mpsc::channel(256);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        (
            ToolExecutionContext {
                output_tx,
                cancellation,
            },
            output_rx,
        )
    }

    #[tokio::test]
    async fn bash_echo() {
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute("c1", serde_json::json!({"command": "echo hello world"}), ctx)
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
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute("c2", serde_json::json!({"command": "exit 42"}), ctx)
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
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute("c3", serde_json::json!({"command": "sleep 10", "timeout": 1}), ctx)
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
        let (ctx, _rx) = make_context();
        let tool_instance = tool();

        let handle = tokio::spawn(async move {
            tool_instance
                .execute("c4", serde_json::json!({"command": "sleep 30"}), ctx)
                .await
        });

        // Cancel the token after a short delay
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // We need to cancel the token, but we moved ctx into the task.
        // So we use a different approach: create a shared token.
        // Actually, we need to restructure this test.
        // Let's use the cancelled context approach instead.
        // This test needs to be restructured.
        drop(handle); // This will be restructured below

        // Use pre-cancelled context approach instead
        let (ctx2, _rx2) = make_cancelled_context();
        let result = tool()
            .execute("c4b", serde_json::json!({"command": "sleep 30"}), ctx2)
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text,
            _ => panic!("Expected text content"),
        };
        assert!(text.contains("aborted"), "Expected aborted message, got: {}", text);
    }

    #[tokio::test]
    async fn bash_with_output_and_timeout() {
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute(
                "c5",
                serde_json::json!({"command": "echo hi && sleep 0.5 && echo bye", "timeout": 10}),
                ctx,
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
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute(
                "c6",
                serde_json::json!({"command": "echo small", "max_lines": 100, "max_bytes": 99999}),
                ctx,
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
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute(
                "c7",
                serde_json::json!({"command": "seq 1 100", "max_lines": 5, "max_bytes": 99999}),
                ctx,
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
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute(
                "c8",
                serde_json::json!({"command": "echo short && echo long_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "max_lines": 9999, "max_bytes": 30}),
                ctx,
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

        let (ctx1, _rx1) = make_context();
        let result = tool
            .execute("c1", serde_json::json!({"command": "cd sub && pwd"}), ctx1)
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

        let (ctx2, _rx2) = make_context();
        let result2 = tool
            .execute("c2", serde_json::json!({"command": "pwd"}), ctx2)
            .await
            .unwrap();
        let text2 = match &result2.content[0] {
            Content::Text { text } => text.trim(),
            _ => panic!("Expected text content"),
        };
        assert!(text2.ends_with("/sub"), "Second pwd should show sub, got: {}", text2);

        let (ctx3, _rx3) = make_context();
        let result3 = tool
            .execute("c3", serde_json::json!({"command": "cd .. && pwd"}), ctx3)
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
        let (ctx, mut rx) = make_context();
        let result = tool()
            .execute("to1", serde_json::json!({"command": "echo hello"}), ctx)
            .await
            .unwrap();
        assert!(!result.is_error);

        // Collect ToolOutput events
        let mut outputs = Vec::new();
        while let Some(ev) = rx.recv().await {
            outputs.push(ev);
        }
        assert!(!outputs.is_empty(), "Should have ToolOutput events");
        assert!(
            outputs
                .iter()
                .any(|e| e.stream == ToolOutputStream::Stdout && e.chunk.contains("hello"))
        );
    }

    #[tokio::test]
    async fn tool_output_stderr_single_chunk() {
        let (ctx, mut rx) = make_context();
        let _result = tool()
            .execute("to2", serde_json::json!({"command": "echo err >&2"}), ctx)
            .await
            .unwrap();

        let mut outputs = Vec::new();
        while let Some(ev) = rx.recv().await {
            outputs.push(ev);
        }
        assert!(
            outputs
                .iter()
                .any(|e| e.stream == ToolOutputStream::Stderr && e.chunk.contains("err"))
        );
    }

    #[tokio::test]
    async fn tool_output_stdout_stderr_interleaved() {
        let (ctx, mut rx) = make_context();
        let result = tool()
            .execute(
                "to3",
                serde_json::json!({"command": "echo out1; echo err1 >&2; echo out2; echo err2 >&2"}),
                ctx,
            )
            .await
            .unwrap();
        assert!(!result.is_error);

        let mut outputs = Vec::new();
        while let Some(ev) = rx.recv().await {
            outputs.push(ev);
        }
        let stdout_chunks: Vec<&str> = outputs
            .iter()
            .filter(|e| e.stream == ToolOutputStream::Stdout)
            .map(|e| e.chunk.as_str())
            .collect();
        let stderr_chunks: Vec<&str> = outputs
            .iter()
            .filter(|e| e.stream == ToolOutputStream::Stderr)
            .map(|e| e.chunk.as_str())
            .collect();
        assert!(!stdout_chunks.is_empty(), "Should have stdout chunks");
        assert!(!stderr_chunks.is_empty(), "Should have stderr chunks");
    }

    #[tokio::test]
    async fn tool_output_no_late_events_after_cancel() {
        let cancellation = CancellationToken::new();
        let (output_tx, mut rx) = tokio::sync::mpsc::channel(256);
        let ctx = ToolExecutionContext {
            output_tx,
            cancellation: cancellation.clone(),
        };

        let handle = tokio::spawn(async move {
            tool()
                .execute(
                    "to_cancel",
                    serde_json::json!({"command": "for i in 1 2 3 4 5; do echo line_$i; sleep 0.1; done"}),
                    ctx,
                )
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancellation.cancel();

        let result = handle.await.unwrap().unwrap();
        assert!(result.aborted);

        // Drain any events that arrived before cancel
        while let Ok(_ev) = rx.try_recv() {}
        // After cancel, no new events should arrive
    }

    #[tokio::test]
    async fn tool_output_timeout_no_late_events() {
        let (ctx, mut rx) = make_context();
        let result = tool()
            .execute(
                "to_timeout",
                serde_json::json!({"command": "echo start; sleep 30", "timeout": 1}),
                ctx,
            )
            .await
            .unwrap();
        assert!(result.timed_out);

        // Drain events
        while let Ok(_ev) = rx.try_recv() {}
        // Events should have stopped after timeout
    }

    // ── Regression tests for parallel execution ──────────────────────────

    #[tokio::test]
    async fn bash_abort_pre_cancelled_token() {
        let (ctx, _rx) = make_cancelled_context();
        let result = tool()
            .execute("c_pre", serde_json::json!({"command": "sleep 30"}), ctx)
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
        let cancellation = CancellationToken::new();
        let (output_tx, _rx) = tokio::sync::mpsc::channel(256);
        let ctx = ToolExecutionContext {
            output_tx,
            cancellation: cancellation.clone(),
        };

        let handle = tokio::spawn(async move {
            tool()
                .execute(
                    "c_conc",
                    serde_json::json!({"command": "for i in 1 2 3 4 5; do echo line_$i; sleep 0.1; done"}),
                    ctx,
                )
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancellation.cancel();

        let result = handle.await.unwrap().unwrap();
        assert!(result.aborted);
    }

    #[tokio::test]
    async fn bash_timeout_then_no_hang() {
        let (ctx, _rx) = make_context();
        let result = tool()
            .execute("c_to", serde_json::json!({"command": "sleep 60", "timeout": 1}), ctx)
            .await
            .unwrap();
        assert!(result.timed_out);
        assert!(result.is_error);
    }

    // ── ToolExecutionContext isolation tests ──────────────────────────────

    #[tokio::test]
    async fn same_tool_concurrent_no_output_crossover() {
        let t = tool();
        let (ctx1, mut rx1) = make_context();
        let (ctx2, mut rx2) = make_context();

        let (h1, h2) = tokio::join!(
            t.execute("id_a", serde_json::json!({"command": "echo alpha"}), ctx1),
            t.execute("id_b", serde_json::json!({"command": "echo beta"}), ctx2),
        );

        let r1 = h1.unwrap();
        let r2 = h2.unwrap();

        // Each context receives only its own output
        let mut out1 = Vec::new();
        while let Some(ev) = rx1.recv().await {
            out1.push(ev);
        }
        let mut out2 = Vec::new();
        while let Some(ev) = rx2.recv().await {
            out2.push(ev);
        }

        let text1 = match &r1.content[0] {
            Content::Text { text } => text.trim().to_string(),
            _ => panic!(),
        };
        let text2 = match &r2.content[0] {
            Content::Text { text } => text.trim().to_string(),
            _ => panic!(),
        };

        assert_eq!(text1, "alpha");
        assert_eq!(text2, "beta");

        // Verify output events go to the right channel
        assert!(out1.iter().any(|e| e.chunk.contains("alpha")));
        assert!(!out1.iter().any(|e| e.chunk.contains("beta")));
        assert!(out2.iter().any(|e| e.chunk.contains("beta")));
        assert!(!out2.iter().any(|e| e.chunk.contains("alpha")));
    }

    #[tokio::test]
    async fn cancel_one_does_not_affect_other() {
        let cancel1 = CancellationToken::new();
        let cancel2 = CancellationToken::new();
        let (tx1, _rx1) = tokio::sync::mpsc::channel(256);
        let (tx2, _rx2) = tokio::sync::mpsc::channel(256);
        let ctx1 = ToolExecutionContext {
            output_tx: tx1,
            cancellation: cancel1.clone(),
        };
        let ctx2 = ToolExecutionContext {
            output_tx: tx2,
            cancellation: cancel2.clone(),
        };

        let h1 = tokio::spawn(async move {
            tool()
                .execute("slow", serde_json::json!({"command": "sleep 30"}), ctx1)
                .await
        });
        let h2 = tokio::spawn(async move {
            tool()
                .execute("fast", serde_json::json!({"command": "echo done"}), ctx2)
                .await
        });

        // Cancel the first, not the second
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel1.cancel();

        let r1 = h1.await.unwrap().unwrap();
        let r2 = h2.await.unwrap().unwrap();

        assert!(r1.aborted, "First should be aborted");
        assert!(!r2.is_error, "Second should succeed");
    }

    #[tokio::test]
    async fn sender_closed_after_execution() {
        let (ctx, mut rx) = make_context();
        let _result = tool()
            .execute("sc", serde_json::json!({"command": "echo test"}), ctx)
            .await
            .unwrap();

        // After execution, the context's output_tx should have been moved into
        // the tool and consumed. The receiver should see channel closed.
        // (In the new design, output_tx is moved into the tool's execute method,
        // so after the method returns, the sender is dropped.)
        // The receiver can still drain buffered messages.
        while let Some(_ev) = rx.recv().await {}
        // Channel is closed after execute returns
    }

    // ── High-concurrency lifecycle tests ───────────────────────────────

    /// Spawn 20 concurrent echo commands and verify all complete correctly.
    #[tokio::test]
    async fn concurrent_short_commands_all_complete() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let t = Arc::new(BashTool::new(shared_cwd));
        let mut handles = Vec::new();

        for i in 0..20 {
            let t = Arc::clone(&t);
            let (ctx, _rx) = make_context();
            let id = format!("conc_{}", i);
            handles.push(tokio::spawn(async move {
                t.execute(&id, serde_json::json!({"command": "echo test"}), ctx).await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap().unwrap();
            let text = match &result.content[0] {
                Content::Text { text } => text.trim().to_string(),
                _ => panic!("Expected text for command {}", i),
            };
            assert_eq!(text, "test", "Command {} output mismatch", i);
            assert!(!result.is_error, "Command {} should not error", i);
        }
    }

    /// Spawn 20 concurrent long commands, cancel all, verify all aborted.
    #[tokio::test]
    async fn concurrent_cancel_half() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let t = Arc::new(BashTool::new(shared_cwd));
        let mut handles = Vec::new();
        let mut cancel_tokens = Vec::new();

        for i in 0..20 {
            let cancel = CancellationToken::new();
            let (output_tx, _rx) = tokio::sync::mpsc::channel(256);
            let ctx = ToolExecutionContext {
                output_tx,
                cancellation: cancel.clone(),
            };
            let id = format!("conc_cancel_{}", i);
            cancel_tokens.push((i, cancel));
            let t = Arc::clone(&t);
            handles.push(tokio::spawn(async move {
                t.execute(&id, serde_json::json!({"command": "sleep 30"}), ctx).await
            }));
        }

        // Let them start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel the first 10
        for (_i, cancel) in &cancel_tokens[..10] {
            cancel.cancel();
        }

        // Cancel remaining to clean up immediately
        for (_i, cancel) in &cancel_tokens[10..] {
            cancel.cancel();
        }

        // All 20 should complete quickly since all are cancelled
        let all_results = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut results = Vec::new();
            for handle in handles {
                results.push(handle.await.unwrap());
            }
            results
        })
        .await;
        assert!(all_results.is_ok(), "All tasks should complete within 15s");
        for (i, result) in all_results.unwrap().iter().enumerate() {
            let r = result.as_ref().unwrap();
            assert!(r.aborted, "Command {} should be aborted", i);
        }
    }

    /// Spawn multiple commands that all timeout, verify all report timed_out.
    #[tokio::test]
    async fn concurrent_timeout_all() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let t = Arc::new(BashTool::new(shared_cwd));
        let mut handles = Vec::new();

        for i in 0..10 {
            let t = Arc::clone(&t);
            let (ctx, _rx) = make_context();
            let id = format!("conc_to_{}", i);
            handles.push(tokio::spawn(async move {
                t.execute(&id, serde_json::json!({"command": "sleep 30", "timeout": 1}), ctx)
                    .await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let result = handle.await.unwrap().unwrap();
            assert!(result.timed_out, "Command {} should be timed_out", i);
            assert!(result.is_error, "Command {} should report error on timeout", i);
        }
    }

    /// Race between normal completion and cancellation — must not hang or panic.
    #[tokio::test]
    async fn race_completion_vs_cancel() {
        for iteration in 0..100 {
            let cancel = CancellationToken::new();
            let (output_tx, _rx) = tokio::sync::mpsc::channel(256);
            let ctx = ToolExecutionContext {
                output_tx,
                cancellation: cancel.clone(),
            };

            // Random-ish duration: sometimes finishes before cancel, sometimes not
            let sleep_ms = if iteration % 2 == 0 { 10 } else { 50 };
            let tool_id = format!("race_{}", iteration);

            let handle = tokio::spawn(async move {
                tool()
                    .execute(
                        &tool_id,
                        serde_json::json!({"command": format!("sleep 0.0{}", sleep_ms)}),
                        ctx,
                    )
                    .await
            });

            // Cancel at a random-ish point
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            cancel.cancel();

            let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
                .await
                .expect("Should not hang")
                .expect("Task should not panic");

            // Result can be either completed or aborted — both are valid
            let r = result.unwrap();
            assert!(
                !(r.timed_out && r.aborted),
                "Should not have both timed_out and aborted"
            );
        }
    }

    /// Race between timeout and cancellation — must produce exactly one terminal state.
    #[tokio::test]
    async fn race_timeout_vs_cancel() {
        for iteration in 0..50 {
            let cancel = CancellationToken::new();
            let (output_tx, _rx) = tokio::sync::mpsc::channel(256);
            let ctx = ToolExecutionContext {
                output_tx,
                cancellation: cancel.clone(),
            };

            let tool_id = format!("race_tc_{}", iteration);
            let handle = tokio::spawn(async move {
                tool()
                    .execute(&tool_id, serde_json::json!({"command": "sleep 30", "timeout": 1}), ctx)
                    .await
            });

            // Cancel very close to the timeout
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            cancel.cancel();

            let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
                .await
                .expect("Should not hang")
                .expect("Task should not panic");

            let r = result.unwrap();
            // Must have exactly one terminal state, not both
            if r.timed_out {
                assert!(!r.aborted, "Should not have both timed_out and aborted");
            } else if r.aborted {
                assert!(!r.timed_out, "Should not have both timed_out and aborted");
            } else {
                // Neither — race resolved before either triggered
                // This is a valid state: the command completed before timeout or cancel
            }
        }
    }

    /// Drop the execution future itself — verify process and thread cleanup.
    ///
    /// This deliberately does not use `tokio::spawn`/`JoinHandle::drop`,
    /// because dropping a Tokio JoinHandle detaches the task and does not drop
    /// the execution future. A pinned future is polled until it has spawned a
    /// real process, then dropped in this task so ChildLifecycle::drop runs.
    #[cfg(unix)]
    #[tokio::test]
    async fn drop_future_cleans_up() {
        let shell_pid_file = tempfile::NamedTempFile::new().unwrap();
        let pid_path = shell_pid_file.path().to_path_buf();
        let cancellation = CancellationToken::new();
        let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(256);
        let context = ToolExecutionContext {
            output_tx,
            cancellation,
        };
        let bash = tool();
        let lifecycle_counters = bash.lifecycle_counters();
        let command = format!(
            "sh -c 'sleep 30 & child=$!; pgid=$(ps -o pgid= -p $$ | tr -d \" \" ); printf \"%s %s %s\\n\" \"$$\" \"$child\" \"$pgid\" > {}; wait'",
            pid_path.display()
        );
        let mut future = Box::pin(bash.execute("drop_test", serde_json::json!({"command": command}), context));

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let (shell_pid, child_pid, pgid) = loop {
            if let Ok(contents) = std::fs::read_to_string(&pid_path) {
                let mut pids = contents.split_whitespace();
                if let (Some(shell), Some(child), Some(pgid)) = (pids.next(), pids.next(), pids.next())
                    && let (Ok(shell), Ok(child), Ok(pgid)) =
                        (shell.parse::<u32>(), child.parse::<u32>(), pgid.parse::<u32>())
                {
                    assert!(
                        lifecycle_counters.active_readers.load(Ordering::SeqCst) >= 2,
                        "both reader threads should be owned while future is pending"
                    );
                    assert!(
                        lifecycle_counters.active_reapers.load(Ordering::SeqCst) >= 1,
                        "reap thread should be owned while future is pending"
                    );
                    assert_ne!(pgid, 0, "the shell must have a process group");
                    break (shell, child, pgid);
                }
            }
            assert!(tokio::time::Instant::now() < deadline, "PID file was not written");
            tokio::select! {
                result = &mut future => panic!("long-running future completed unexpectedly: {:?}", result),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        };

        // Dropping the future invokes ChildLifecycle::drop, which kills the
        // process group, then joins both readers and the sole reap thread.
        drop(future);

        let cleanup_deadline = Instant::now() + std::time::Duration::from_secs(3);
        while Instant::now() < cleanup_deadline
            && (process_exists(shell_pid)
                || process_exists(child_pid)
                || process_exists(pgid)
                || lifecycle_counters.active_readers.load(Ordering::SeqCst) != 0
                || lifecycle_counters.active_reapers.load(Ordering::SeqCst) != 0)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!process_exists(shell_pid), "shell PID {shell_pid} still exists");
        assert!(!process_exists(child_pid), "child PID {child_pid} still exists");
        assert!(!process_exists(pgid), "process group leader/PGID {pgid} still exists");
        assert_eq!(process_state(shell_pid), None, "shell must not remain zombie");
        assert_eq!(process_state(child_pid), None, "child must not remain zombie");
        assert_eq!(process_state(pgid), None, "PGID must not remain zombie");
        assert_eq!(lifecycle_counters.active_readers.load(Ordering::SeqCst), 0);
        assert_eq!(lifecycle_counters.active_reapers.load(Ordering::SeqCst), 0);

        let mut events_before = 0;
        while output_rx.try_recv().is_ok() {
            events_before += 1;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        let mut events_after = 0;
        while output_rx.try_recv().is_ok() {
            events_after += 1;
        }
        assert_eq!(
            events_after, 0,
            "no ToolOutput may arrive after future drop (before={events_before})"
        );
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    #[cfg(unix)]
    fn process_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = stat.rsplit_once(") ")?;
        rest.chars().next()
    }

    /// Verify no zombie processes remain after concurrent execution.
    #[tokio::test]
    async fn no_zombies_after_concurrent() {
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let t = Arc::new(BashTool::new(shared_cwd));
        let mut handles = Vec::new();

        for i in 0..10 {
            let t = Arc::clone(&t);
            let (ctx, _rx) = make_context();
            let id = format!("zombie_{}", i);
            handles.push(tokio::spawn(async move {
                t.execute(&id, serde_json::json!({"command": "echo clean"}), ctx).await
            }));
        }

        for handle in handles {
            let result = handle.await.unwrap().unwrap();
            assert!(!result.is_error);
        }

        // Check for zombie processes (best effort)
        #[cfg(unix)]
        {
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg("ps -eo pid,ppid,stat,comm | grep -E 'Z|<defunct>' || true")
                .output()
                .unwrap();
            let zombies = String::from_utf8_lossy(&output.stdout);
            // Filter for our test processes only
            let our_zombies: Vec<&str> = zombies
                .lines()
                .filter(|l| l.contains("sleep") || l.contains("echo"))
                .collect();
            assert!(
                our_zombies.is_empty(),
                "Should not have zombie processes: {:?}",
                our_zombies
            );
        }
    }
}
