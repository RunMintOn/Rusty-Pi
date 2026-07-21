//! Stateful print frontend with injectable output sink.
//!
//! This module provides [`PrintFrontend`], a frontend that consumes
//! [`AgentEvent`] and writes to configurable output sinks. It tracks
//! per-run state (run ID, tool states, terminal event) and provides
//! a [`drive_print_run`] function for coordinating agent execution.

use crate::agent::events::{AgentEvent, AgentRunError, RunId, ToolOutputStream};
use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;
use crate::coding_agent::command::CommandResult;
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

// ── Output Sink ─────────────────────────────────────────────────────────────

/// Abstraction for writing to stdout and stderr.
///
/// Production uses real `io::stdout` / `io::stderr`. Tests use
/// [`MemoryOutput`] for precise assertions.
pub trait FrontendOutput {
    /// Write to stdout.
    fn write_stdout(&mut self, text: &str) -> io::Result<()>;
    /// Write to stderr.
    fn write_stderr(&mut self, text: &str) -> io::Result<()>;
    /// Flush stdout.
    fn flush_stdout(&mut self) -> io::Result<()>;
    /// Flush stderr.
    fn flush_stderr(&mut self) -> io::Result<()>;
}

/// Production output sink connected to real stdout/stderr.
pub struct RealOutput;

impl FrontendOutput for RealOutput {
    fn write_stdout(&mut self, text: &str) -> io::Result<()> {
        io::stdout().write_all(text.as_bytes())
    }
    fn write_stderr(&mut self, text: &str) -> io::Result<()> {
        io::stderr().write_all(text.as_bytes())
    }
    fn flush_stdout(&mut self) -> io::Result<()> {
        io::stdout().flush()
    }
    fn flush_stderr(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

/// In-memory output sink for testing.
///
/// Captures stdout and stderr as separate `Vec<u8>` buffers, enabling
/// precise assertions on output content, ordering, and separation.
#[derive(Debug, Default)]
pub struct MemoryOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl MemoryOutput {
    /// Create a new empty memory output.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get stdout content as a string (lossy).
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).to_string()
    }

    /// Get stderr content as a string (lossy).
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).to_string()
    }

    /// Clear both buffers.
    pub fn clear(&mut self) {
        self.stdout.clear();
        self.stderr.clear();
    }
}

impl FrontendOutput for MemoryOutput {
    fn write_stdout(&mut self, text: &str) -> io::Result<()> {
        self.stdout.extend_from_slice(text.as_bytes());
        Ok(())
    }
    fn write_stderr(&mut self, text: &str) -> io::Result<()> {
        self.stderr.extend_from_slice(text.as_bytes());
        Ok(())
    }
    fn flush_stdout(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn flush_stderr(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that always fails, for testing error propagation.
pub struct FailingOutput;

impl FrontendOutput for FailingOutput {
    fn write_stdout(&mut self, _text: &str) -> io::Result<()> {
        Err(io::Error::other("stdout write failed"))
    }
    fn write_stderr(&mut self, _text: &str) -> io::Result<()> {
        Err(io::Error::other("stderr write failed"))
    }
    fn flush_stdout(&mut self) -> io::Result<()> {
        Err(io::Error::other("stdout flush failed"))
    }
    fn flush_stderr(&mut self) -> io::Result<()> {
        Err(io::Error::other("stderr flush failed"))
    }
}

// ── Tool State ──────────────────────────────────────────────────────────────

/// Per-tool state tracked by the frontend.
#[derive(Debug, Clone)]
pub struct ToolState {
    /// Tool name.
    pub name: String,
    /// When the tool started executing.
    pub started_at: Instant,
    /// Whether streaming output was observed for this tool.
    pub saw_stream_output: bool,
}

// ── Stateful PrintFrontend ──────────────────────────────────────────────────

/// A stateful frontend that consumes [`AgentEvent`] and writes to
/// configurable output sinks.
///
/// Tracks per-run state to:
/// - Filter stale `RunId` events
/// - Prevent duplicate terminal events
/// - Track tool states for duration and dedup
/// - Ensure output goes to the correct stream (stdout vs stderr)
pub struct PrintFrontend<O: FrontendOutput = RealOutput> {
    output: O,
    /// Current run ID being processed.
    current_run_id: Option<RunId>,
    /// Whether we've seen any assistant text output this run.
    assistant_output_started: bool,
    /// Tool states indexed by tool_call_id.
    tool_states: HashMap<String, ToolState>,
    /// Whether we've seen a terminal event (RunFinished/RunAborted/RunFailed).
    terminal_event_seen: bool,
}

impl PrintFrontend {
    /// Create a new PrintFrontend with real stdout/stderr output.
    pub fn new() -> Self {
        Self::with_output(RealOutput)
    }
}

impl Default for PrintFrontend {
    fn default() -> Self {
        Self::new()
    }
}

impl<O: FrontendOutput> PrintFrontend<O> {
    /// Create a PrintFrontend with a custom output sink.
    pub fn with_output(output: O) -> Self {
        Self {
            output,
            current_run_id: None,
            assistant_output_started: false,
            tool_states: HashMap::new(),
            terminal_event_seen: false,
        }
    }

    /// Get a reference to the output sink.
    pub fn output(&self) -> &O {
        &self.output
    }

    /// Get a mutable reference to the output sink.
    pub fn output_mut(&mut self) -> &mut O {
        &mut self.output
    }

    /// Process a single agent event.
    ///
    /// Events from a stale RunId are ignored. Duplicate terminal events
    /// are suppressed.
    pub fn handle_event(&mut self, event: &AgentEvent) -> io::Result<()> {
        match event {
            AgentEvent::RunStarted { run_id } => {
                // Initialize or reset state for this run
                self.current_run_id = Some(*run_id);
                self.assistant_output_started = false;
                self.tool_states.clear();
                self.terminal_event_seen = false;
                Ok(())
            }
            AgentEvent::TextDelta { run_id, text } if self.accepts(*run_id) => {
                self.assistant_output_started = true;
                self.output.write_stdout(text)?;
                self.output.flush_stdout()
            }
            AgentEvent::ThinkingDelta { run_id, text } if self.accepts(*run_id) => {
                self.output.write_stderr(&format!("[thinking] {}", text))?;
                self.output.flush_stderr()
            }
            AgentEvent::ToolStarted {
                run_id,
                tool_call_id,
                name,
                arguments,
            } if self.accepts(*run_id) => {
                // Don't create duplicate state for the same tool_call_id
                if !self.tool_states.contains_key(tool_call_id) {
                    let args_str = if arguments.is_object() && arguments.as_object().map_or(false, |m| m.is_empty()) {
                        String::new()
                    } else {
                        format!(" {}", arguments)
                    };
                    self.output.write_stderr(&format!("\n⚙ {}{}\n", name, args_str))?;
                    self.output.flush_stderr()?;
                    self.tool_states.insert(
                        tool_call_id.clone(),
                        ToolState {
                            name: name.clone(),
                            started_at: Instant::now(),
                            saw_stream_output: false,
                        },
                    );
                }
                Ok(())
            }
            AgentEvent::ToolOutput {
                run_id,
                tool_call_id,
                stream,
                chunk,
            } if self.accepts(*run_id) => {
                // Mark that this tool has produced streaming output
                if let Some(state) = self.tool_states.get_mut(tool_call_id) {
                    state.saw_stream_output = true;
                }
                match stream {
                    ToolOutputStream::Stdout => {
                        self.output.write_stderr(chunk)?;
                        self.output.flush_stderr()?;
                    }
                    ToolOutputStream::Stderr => {
                        self.output.write_stderr(chunk)?;
                        self.output.flush_stderr()?;
                    }
                }
                Ok(())
            }
            AgentEvent::ToolFinished {
                run_id,
                tool_call_id,
                name,
                result,
            } if self.accepts(*run_id) => self.print_tool_finished(tool_call_id, name, result),
            AgentEvent::ProviderError { run_id, error } if self.accepts(*run_id) => {
                self.output
                    .write_stderr(&format!("\n❌ Provider error: {}", error.message))?;
                self.output.flush_stderr()
            }
            AgentEvent::RunAborted { run_id } if self.accepts(*run_id) => {
                if !self.terminal_event_seen {
                    self.terminal_event_seen = true;
                    self.output.write_stderr("\n⏹ Run aborted\n")?;
                    self.output.flush_stderr()?;
                }
                Ok(())
            }
            AgentEvent::RunFailed { run_id, error } if self.accepts(*run_id) => {
                if !self.terminal_event_seen {
                    self.terminal_event_seen = true;
                    self.output
                        .write_stderr(&format!("\n❌ Run failed [{}]: {}\n", error.phase, error.message))?;
                    self.output.flush_stderr()?;
                }
                Ok(())
            }
            AgentEvent::RunFinished {
                run_id, stop_reason, ..
            } if self.accepts(*run_id) => {
                if !self.terminal_event_seen {
                    self.terminal_event_seen = true;
                    if *stop_reason == StopReason::Length {
                        self.output.write_stderr("\n⚠ Response truncated (length limit)\n")?;
                        self.output.flush_stderr()?;
                    }
                }
                Ok(())
            }
            _ => Ok(()), // Stale event — ignore
        }
    }

    /// Handle a command result from slash commands.
    pub fn handle_command_result(&mut self, result: &CommandResult) -> io::Result<()> {
        match result {
            CommandResult::Message(message) => {
                self.output.write_stdout(message)?;
                self.output.write_stdout("\n")?;
                self.output.flush_stdout()
            }
            CommandResult::Error(message) => {
                self.output.write_stderr(message)?;
                self.output.write_stderr("\n")?;
                self.output.flush_stderr()
            }
            CommandResult::Help(items) => {
                self.output.write_stdout("\n  Commands:\n")?;
                for item in items {
                    self.output
                        .write_stdout(&format!("    /{:<12} {}\n", item.name, item.description))?;
                }
                self.output.write_stdout("\n  Tips:\n")?;
                self.output
                    .write_stdout("    - Up/down arrows navigate command history\n")?;
                self.output.write_stdout("    - Ctrl+C at prompt exits\n")?;
                self.output
                    .write_stdout("    - Ctrl+C during agent run aborts the current round\n")?;
                self.output
                    .write_stdout("    - Type any text to chat with the agent\n")?;
                self.output.flush_stdout()
            }
            CommandResult::ModelChanged { model } => {
                self.output.write_stdout(&format!("Switched to {}\n", model))?;
                self.output.flush_stdout()
            }
            CommandResult::Sessions(sessions) => {
                if sessions.is_empty() {
                    self.output.write_stdout("No sessions found.\n")?;
                } else {
                    self.output.write_stdout("Available sessions:\n")?;
                    for s in sessions {
                        self.output.write_stdout(&format!(
                            "  {} | model: {} | msgs: {} | created: {}\n",
                            s.id, s.model, s.msg_count, s.created
                        ))?;
                    }
                }
                self.output.flush_stdout()
            }
            CommandResult::Quit | CommandResult::Noop => Ok(()),
        }
    }

    /// Returns true if the event's run_id matches the current run.
    fn accepts(&self, run_id: RunId) -> bool {
        self.current_run_id == Some(run_id)
    }

    /// Print tool finished with dedup logic.
    fn print_tool_finished(&mut self, tool_call_id: &str, name: &str, result: &AgentToolResult) -> io::Result<()> {
        let saw_stream = self
            .tool_states
            .get(tool_call_id)
            .map_or(false, |s| s.saw_stream_output);
        let duration = self.tool_states.get(tool_call_id).map(|s| s.started_at.elapsed());

        if result.is_error {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| match c {
                    crate::ai::types::Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .next()
                .unwrap_or("unknown error");

            let duration_str = duration
                .map(|d| format!(" in {:.1}s", d.as_secs_f64()))
                .unwrap_or_default();

            if let Some(code) = result.exit_code {
                self.output.write_stderr(&format!(
                    "  ❌ {} exit code {}{} — {}\n",
                    name, code, duration_str, error_text
                ))?;
            } else if result.timed_out {
                self.output
                    .write_stderr(&format!("  ⏰ {} timed out{} — {}\n", name, duration_str, error_text))?;
            } else if result.aborted {
                self.output
                    .write_stderr(&format!("  ⏹ {} aborted{} — {}\n", name, duration_str, error_text))?;
            } else {
                self.output
                    .write_stderr(&format!("  ❌ {} error{} — {}\n", name, duration_str, error_text))?;
            }
            self.output.flush_stderr()
        } else if saw_stream {
            // Tool already produced streaming output — show only summary
            let duration_str = duration
                .map(|d| format!(" in {:.1}s", d.as_secs_f64()))
                .unwrap_or_default();
            self.output
                .write_stderr(&format!("  ✅ {} done{}\n", name, duration_str))?;
            self.output.flush_stderr()
        } else {
            // No streaming output — show first line of result
            let output_line = result
                .content
                .iter()
                .filter_map(|c| match c {
                    crate::ai::types::Content::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .next()
                .unwrap_or("");

            let first_line = output_line.lines().next().unwrap_or("");
            if !first_line.is_empty() {
                let display = if first_line.len() > 80 {
                    format!("{}...", &first_line[..77])
                } else {
                    first_line.to_string()
                };
                let duration_str = duration
                    .map(|d| format!(" in {:.1}s", d.as_secs_f64()))
                    .unwrap_or_default();
                self.output
                    .write_stderr(&format!("  ✅ {}{}: {}\n", name, duration_str, display))?;
                self.output.flush_stderr()
            } else {
                Ok(())
            }
        }
    }
}

// ── Print Run Driver ────────────────────────────────────────────────────────

/// Outcome of a print run.
#[derive(Debug, Clone, PartialEq)]
pub enum PrintRunOutcome {
    /// Run finished with the given stop reason.
    Finished(StopReason),
    /// Run was cancelled by the user.
    Aborted,
    /// Run failed with an internal error.
    Failed(AgentRunError),
}

/// Drive a single agent run, coordinating:
/// - Agent future execution
/// - Event channel processing
/// - Ctrl+C cancellation
/// - PrintFrontend output
///
/// Returns [`PrintRunOutcome`] indicating how the run ended.
pub async fn drive_print_run<O: FrontendOutput>(
    agent: &mut crate::agent::engine::Agent,
    prompt: &str,
    frontend: &mut PrintFrontend<O>,
    run_token: CancellationToken,
) -> PrintRunOutcome {
    // Set up event channel
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    agent.set_event_sender(event_tx);
    agent.set_abort_flag(run_token.clone());

    let mut outcome = None;

    // Run the agent future
    let run_future = agent.run(prompt);
    tokio::pin!(run_future);

    loop {
        tokio::select! {
            result = &mut run_future => {
                // Agent finished
                match result {
                    Ok(()) => {}
                    Err(e) => {
                        if outcome.is_none() {
                            eprintln!("\n[error] {}", e);
                        }
                    }
                }
                // Drain remaining events
                while let Ok(event) = event_rx.try_recv() {
                    let _ = frontend.handle_event(&event);
                    match &event {
                        AgentEvent::RunFinished { stop_reason, .. } => {
                            outcome = Some(PrintRunOutcome::Finished(stop_reason.clone()));
                        }
                        AgentEvent::RunAborted { .. } => {
                            outcome = Some(PrintRunOutcome::Aborted);
                        }
                        AgentEvent::RunFailed { error, .. } => {
                            outcome = Some(PrintRunOutcome::Failed(error.clone()));
                        }
                        _ => {}
                    }
                }
                break;
            }
            event = event_rx.recv() => {
                if let Some(event) = event {
                    let _ = frontend.handle_event(&event);
                    match &event {
                        AgentEvent::RunFinished { stop_reason, .. } => {
                            outcome = Some(PrintRunOutcome::Finished(stop_reason.clone()));
                        }
                        AgentEvent::RunAborted { .. } => {
                            outcome = Some(PrintRunOutcome::Aborted);
                        }
                        AgentEvent::RunFailed { error, .. } => {
                            outcome = Some(PrintRunOutcome::Failed(error.clone()));
                        }
                        _ => {}
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                if !run_token.is_cancelled() {
                    run_token.cancel();
                    // Don't break — continue processing events until the agent settles
                }
            }
        }
    }

    outcome.unwrap_or(PrintRunOutcome::Aborted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::{AgentEvent, ProviderError, RunId};
    use crate::ai::types::{Content, StopReason};

    #[test]
    fn memory_output_captures_stdout_and_stderr() {
        let mut out = MemoryOutput::new();
        out.write_stdout("hello").unwrap();
        out.write_stderr("world").unwrap();
        assert_eq!(out.stdout_str(), "hello");
        assert_eq!(out.stderr_str(), "world");
    }

    #[test]
    fn memory_output_clear() {
        let mut out = MemoryOutput::new();
        out.write_stdout("a").unwrap();
        out.write_stderr("b").unwrap();
        out.clear();
        assert!(out.stdout.is_empty());
        assert!(out.stderr.is_empty());
    }

    #[test]
    fn failing_output_returns_error() {
        let mut out = FailingOutput;
        assert!(out.write_stdout("x").is_err());
        assert!(out.write_stderr("x").is_err());
        assert!(out.flush_stdout().is_err());
        assert!(out.flush_stderr().is_err());
    }

    #[test]
    fn print_frontend_text_delta_goes_to_stdout() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::TextDelta {
                run_id: RunId::new(1),
                text: "hello".into(),
            })
            .unwrap();
        assert_eq!(frontend.output().stdout_str(), "hello");
        assert!(frontend.output().stderr.is_empty());
    }

    #[test]
    fn print_frontend_tool_started_goes_to_stderr() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls"}),
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr_str().contains("bash"));
    }

    #[test]
    fn print_frontend_tool_output_goes_to_stderr() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                stream: ToolOutputStream::Stdout,
                chunk: "file.txt".into(),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                stream: ToolOutputStream::Stderr,
                chunk: "warning".into(),
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr_str().contains("file.txt"));
        assert!(frontend.output().stderr_str().contains("warning"));
    }

    #[test]
    fn print_frontend_tool_finished_no_streaming_shows_summary() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "read".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "read".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "file contents here".into(),
                    }],
                    ..Default::default()
                },
            })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("✅"));
        assert!(stderr.contains("read"));
    }

    #[test]
    fn print_frontend_tool_finished_with_streaming_shows_only_summary() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                stream: ToolOutputStream::Stdout,
                chunk: "output data".into(),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "output data".into(),
                    }],
                    ..Default::default()
                },
            })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        // Should not repeat "output data" in the ToolFinished output
        // (only the streaming chunk should have been printed)
        assert!(stderr.contains("done"));
    }

    #[test]
    fn print_frontend_provider_error_goes_to_stderr() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ProviderError {
                run_id: RunId::new(1),
                error: ProviderError {
                    reason: StopReason::Error,
                    message: "API limit".into(),
                },
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr_str().contains("Provider error"));
        assert!(frontend.output().stderr_str().contains("API limit"));
    }

    #[test]
    fn print_frontend_run_aborted_once() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(1) })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("Run aborted").count(), 1);
    }

    #[test]
    fn print_frontend_run_failed_includes_phase_and_message() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunFailed {
                run_id: RunId::new(1),
                error: AgentRunError {
                    phase: crate::agent::events::AgentRunPhase::ProviderStart,
                    message: "connection refused".into(),
                },
            })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("provider start"));
        assert!(stderr.contains("connection refused"));
    }

    #[test]
    fn print_frontend_stale_event_ignored() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(2) })
            .unwrap();
        // Stale event from run 1
        frontend
            .handle_event(&AgentEvent::TextDelta {
                run_id: RunId::new(1),
                text: "old".into(),
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
    }

    #[test]
    fn print_frontend_duplicate_tool_started_ignored() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        // Should only have one "bash" in stderr (the tool name)
        let count = frontend.output().stderr_str().matches("bash").count();
        assert_eq!(count, 1, "Should only have one tool name in stderr");
    }

    #[test]
    fn print_frontend_run_finished_success_no_extra_output() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunFinished {
                run_id: RunId::new(1),
                stop_reason: StopReason::Stop,
            })
            .unwrap();
        // Success run_finished should not produce any output
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr.is_empty());
    }

    #[test]
    fn print_frontend_run_finished_length_goes_to_stderr() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunFinished {
                run_id: RunId::new(1),
                stop_reason: StopReason::Length,
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr_str().contains("truncated"));
    }

    #[test]
    fn print_frontend_duplicate_terminal_event_suppressed() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(1) })
            .unwrap();
        // Second RunAborted should be suppressed
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(1) })
            .unwrap();
        let count = frontend.output().stderr_str().matches("aborted").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn print_frontend_run_reset_on_new_run_started() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        // First run
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(1) })
            .unwrap();
        // Second run — state should be reset, new abort is legitimate
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(2) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(2) })
            .unwrap();
        // Both runs produce abort messages — that's correct behavior
        let count = frontend.output().stderr_str().matches("aborted").count();
        assert_eq!(count, 2);
        // But a duplicate RunAborted for run 2 should be suppressed
        frontend
            .handle_event(&AgentEvent::RunAborted { run_id: RunId::new(2) })
            .unwrap();
        let count = frontend.output().stderr_str().matches("aborted").count();
        assert_eq!(count, 2, "Duplicate terminal event should be suppressed");
    }

    #[test]
    fn print_frontend_command_result_message_goes_to_stdout() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_command_result(&CommandResult::Message("hello".into()))
            .unwrap();
        assert_eq!(frontend.output().stdout_str(), "hello\n");
        assert!(frontend.output().stderr.is_empty());
    }

    #[test]
    fn print_frontend_command_result_error_goes_to_stderr() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_command_result(&CommandResult::Error("oops".into()))
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert_eq!(frontend.output().stderr_str(), "oops\n");
    }

    #[test]
    fn print_frontend_tool_error_with_exit_code() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_1".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "command failed".into(),
                    }],
                    is_error: true,
                    exit_code: Some(127),
                    ..Default::default()
                },
            })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("127"));
        assert!(stderr.contains("command failed"));
    }

    #[test]
    fn print_frontend_stdout_write_error_propagates() {
        let mut frontend = PrintFrontend::with_output(FailingOutput);
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        let result = frontend.handle_event(&AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "x".into(),
        });
        assert!(result.is_err());
    }

    #[test]
    fn print_frontend_stderr_write_error_propagates() {
        let mut frontend = PrintFrontend::with_output(FailingOutput);
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        let result = frontend.handle_event(&AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        assert!(result.is_err());
    }

    #[test]
    fn print_frontend_tool_finished找不到started_shows_summary() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        // ToolFinished without a preceding ToolStarted
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_orphan".into(),
                name: "read".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "file content".into(),
                    }],
                    ..Default::default()
                },
            })
            .unwrap();
        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("read"));
    }
}
