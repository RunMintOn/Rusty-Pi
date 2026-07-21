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
use std::borrow::Cow;
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
    /// Whether the completion event for this tool has been rendered.
    pub finished: bool,
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

    /// Report an agent error that arrived without a corresponding event.
    ///
    /// Normal agent failures are represented by `AgentEvent::RunFailed` and
    /// therefore go through `handle_event`. This fallback is for failures
    /// that happen before the event stream is initialized; it still uses the
    /// configured sink rather than writing to a global stderr handle.
    pub fn report_run_error(&mut self, error: &AgentRunError) -> io::Result<()> {
        if !self.terminal_event_seen {
            self.terminal_event_seen = true;
            self.output
                .write_stderr(&format!("\n❌ Run failed [{}]: {}\n", error.phase, error.message))?;
            self.output.flush_stderr()?;
        }
        Ok(())
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
            AgentEvent::TextDelta { run_id, text } if self.accepts_active(*run_id) => {
                self.assistant_output_started = true;
                self.output.write_stdout(text)?;
                self.output.flush_stdout()
            }
            AgentEvent::ThinkingDelta { run_id, text } if self.accepts_active(*run_id) => {
                self.output.write_stderr(&format!("[thinking] {}", text))?;
                self.output.flush_stderr()
            }
            AgentEvent::ToolStarted {
                run_id,
                tool_call_id,
                name,
                arguments,
            } if self.accepts_active(*run_id) => {
                // Don't create duplicate state for the same tool_call_id
                if !self.tool_states.contains_key(tool_call_id) {
                    let args_str = if arguments.is_object() && arguments.as_object().is_some_and(|m| m.is_empty()) {
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
                            finished: false,
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
            } if self.accepts_active(*run_id) => {
                // Mark that this tool has produced streaming output
                let Some(state) = self.tool_states.get_mut(tool_call_id) else {
                    // Output without a start event is stale or malformed. It
                    // must not resurrect a tool lifecycle.
                    return Ok(());
                };
                if !state.finished {
                    state.saw_stream_output = true;
                    match stream {
                        ToolOutputStream::Stdout | ToolOutputStream::Stderr => {
                            self.output.write_stderr(chunk)?;
                            self.output.flush_stderr()?;
                        }
                    }
                }
                Ok(())
            }
            AgentEvent::ToolFinished {
                run_id,
                tool_call_id,
                name,
                result,
            } if self.accepts_active(*run_id) => self.print_tool_finished(tool_call_id, name, result),
            AgentEvent::ProviderError { run_id, error } if self.accepts_active(*run_id) => {
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

    /// Returns true for events that may still produce output in the current run.
    fn accepts_active(&self, run_id: RunId) -> bool {
        self.accepts(run_id) && !self.terminal_event_seen
    }

    /// Print tool finished with dedup logic.
    fn print_tool_finished(&mut self, tool_call_id: &str, name: &str, result: &AgentToolResult) -> io::Result<()> {
        let (saw_stream, duration) = match self.tool_states.get_mut(tool_call_id) {
            Some(state) if state.finished => return Ok(()),
            Some(state) => {
                state.finished = true;
                (state.saw_stream_output, Some(state.started_at.elapsed()))
            }
            None => {
                // A completion without a start is still rendered once, but it
                // establishes a finished state so later output cannot leak.
                self.tool_states.insert(
                    tool_call_id.to_string(),
                    ToolState {
                        name: name.to_string(),
                        started_at: Instant::now(),
                        saw_stream_output: false,
                        finished: true,
                    },
                );
                (false, None)
            }
        };

        let duration_str = duration
            .map(|d| format!(" in {:.1}s", d.as_secs_f64()))
            .unwrap_or_default();

        let nonzero_exit_code = result.exit_code.filter(|code| *code != 0);
        let is_failure = result.is_error || nonzero_exit_code.is_some() || result.timed_out || result.aborted;

        if saw_stream {
            // Streaming output has already shown the result content. Only the
            // stateful completion summary is rendered, including error state.
            let summary = if is_failure {
                if result.aborted {
                    format!("  ⏹ {} aborted{}\n", name, duration_str)
                } else if result.timed_out {
                    format!("  ⏰ {} timed out{}\n", name, duration_str)
                } else if let Some(code) = nonzero_exit_code {
                    format!("  ❌ {} exit code {}{}\n", name, code, duration_str)
                } else {
                    format!("  ❌ {} error{}\n", name, duration_str)
                }
            } else {
                format!("  ✅ {} done{}\n", name, duration_str)
            };
            self.output.write_stderr(&summary)?;
            self.output.flush_stderr()
        } else if is_failure {
            // Without streaming, show a bounded first-line error summary.
            let error_text = first_text(result)
                .and_then(|text| text.lines().next())
                .filter(|text| !text.is_empty())
                .unwrap_or("unknown error");
            let display = truncate_chars(error_text, 80);
            let prefix = if result.aborted {
                format!("  ⏹ {} aborted{}", name, duration_str)
            } else if result.timed_out {
                format!("  ⏰ {} timed out{}", name, duration_str)
            } else if let Some(code) = nonzero_exit_code {
                format!("  ❌ {} exit code {}{}", name, code, duration_str)
            } else {
                format!("  ❌ {} error{}", name, duration_str)
            };
            self.output.write_stderr(&format!("{} — {}\n", prefix, display))?;
            self.output.flush_stderr()
        } else {
            // No streaming output — show a bounded first line of the result.
            let summary = match first_text(result).and_then(|text| text.lines().next()) {
                Some(first_line) if !first_line.is_empty() => {
                    let display = truncate_chars(first_line, 80);
                    format!("  ✅ {}{}: {}\n", name, duration_str, display)
                }
                _ => format!("  ✅ {} done{}\n", name, duration_str),
            };
            self.output.write_stderr(&summary)?;
            self.output.flush_stderr()
        }
    }
}

fn first_text(result: &AgentToolResult) -> Option<&str> {
    result.content.iter().find_map(|content| match content {
        crate::ai::types::Content::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

/// Truncate text without ever splitting a UTF-8 code point.
fn truncate_chars<'a>(text: &'a str, max_chars: usize) -> Cow<'a, str> {
    if text.chars().count() <= max_chars {
        return Cow::Borrowed(text);
    }
    if max_chars <= 3 {
        return Cow::Owned(text.chars().take(max_chars).collect());
    }
    let prefix: String = text.chars().take(max_chars - 3).collect();
    Cow::Owned(format!("{}...", prefix))
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
/// Returns [`PrintRunOutcome`] indicating how the run ended. Output failures
/// are returned after the agent future has settled.
pub async fn drive_print_run<O: FrontendOutput>(
    agent: &mut crate::agent::engine::Agent,
    prompt: &str,
    frontend: &mut PrintFrontend<O>,
    run_token: CancellationToken,
) -> io::Result<PrintRunOutcome> {
    // Set up event channel
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    agent.set_event_sender(event_tx);
    agent.set_abort_flag(run_token.clone());

    let mut outcome = None;
    let mut output_error: Option<io::Error> = None;

    // Run the agent future
    let run_future = agent.run(prompt);
    tokio::pin!(run_future);

    loop {
        tokio::select! {
            result = &mut run_future => {
                // Agent finished
                // (errors are captured as RunFailed events by the agent)
                // Drain remaining events
                while let Ok(event) = event_rx.try_recv() {
                    if output_error.is_none()
                        && let Err(error) = frontend.handle_event(&event)
                    {
                        run_token.cancel();
                        output_error = Some(error);
                    }
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
                // If no terminal event was seen, the agent failed before
                // reaching one of its normal terminal-event paths.
                // Report through the frontend output sink.
                if outcome.is_none() {
                    if let Err(e) = result {
                        let error = crate::agent::events::AgentRunError {
                            phase: crate::agent::events::AgentRunPhase::AgentLoop,
                            message: e.to_string(),
                        };
                        if output_error.is_none()
                            && let Err(output) = frontend.report_run_error(&error)
                        {
                            output_error = Some(output);
                        }
                        outcome = Some(PrintRunOutcome::Failed(error));
                    } else {
                        // Agent returned Ok but no terminal event — shouldn't happen
                        outcome = Some(PrintRunOutcome::Aborted);
                    }
                }
                break;
            }
            event = event_rx.recv() => {
                if let Some(event) = event {
                    if output_error.is_none()
                        && let Err(error) = frontend.handle_event(&event)
                    {
                        // Keep the agent alive until it settles. Its provider
                        // and tool tasks own their cleanup obligations.
                        run_token.cancel();
                        output_error = Some(error);
                    }
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

    if let Some(error) = output_error {
        Err(error)
    } else {
        Ok(outcome.unwrap_or(PrintRunOutcome::Aborted))
    }
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
    fn zero_exit_code_without_streaming_is_success() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_zero".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_zero".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text { text: "success".into() }],
                    exit_code: Some(0),
                    is_error: false,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("✅").count(), 1);
        assert!(!stderr.contains("❌"));
        assert!(!stderr.contains("exit code 0"));
        assert_eq!(stderr.matches("success").count(), 1);
    }

    #[test]
    fn zero_exit_code_with_streaming_is_success() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_zero_stream".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_zero_stream".into(),
                stream: ToolOutputStream::Stdout,
                chunk: "streamed success".into(),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_zero_stream".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "streamed success".into(),
                    }],
                    exit_code: Some(0),
                    is_error: false,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("streamed success").count(), 1);
        assert!(stderr.contains("✅ bash done"));
        assert!(!stderr.contains("❌"));
        assert!(!stderr.contains("exit code 0"));
    }

    #[test]
    fn nonzero_exit_code_is_failure() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_nonzero".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text { text: "failed".into() }],
                    exit_code: Some(2),
                    is_error: true,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("❌"));
        assert!(stderr.contains("exit code 2"));
        assert!(!stderr.contains("✅"));
    }

    #[test]
    fn timeout_takes_priority_over_exit_code() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_timeout_exit".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "timed out".into(),
                    }],
                    exit_code: Some(124),
                    timed_out: true,
                    is_error: true,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("timed out"));
        assert!(!stderr.contains("exit code 124"));
    }

    #[test]
    fn abort_takes_priority_over_exit_code() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_abort_exit".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text { text: "aborted".into() }],
                    exit_code: Some(130),
                    aborted: true,
                    is_error: true,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("aborted"));
        assert!(!stderr.contains("exit code 130"));
    }

    #[test]
    fn generic_error_without_code_or_state_is_failure() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_generic_error".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "generic failure".into(),
                    }],
                    is_error: true,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert!(stderr.contains("❌ bash error"));
        assert!(stderr.contains("generic failure"));
        assert!(!stderr.contains("✅"));
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
    fn print_frontend_report_run_error_uses_output_sink() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .report_run_error(&AgentRunError {
                phase: crate::agent::events::AgentRunPhase::AgentLoop,
                message: "internal failure".into(),
            })
            .unwrap();
        assert!(frontend.output().stdout.is_empty());
        assert!(frontend.output().stderr_str().contains("internal failure"));
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

    #[test]
    fn duplicate_tool_finished_emits_one_summary() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_duplicate".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        let finished = AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_duplicate".into(),
            name: "bash".into(),
            result: AgentToolResult::default(),
        };
        frontend.handle_event(&finished).unwrap();
        frontend.handle_event(&finished).unwrap();

        assert_eq!(frontend.output().stderr_str().matches("✅").count(), 1);
    }

    #[test]
    fn tool_output_after_finished_is_ignored() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_late".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_late".into(),
                name: "bash".into(),
                result: AgentToolResult::default(),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_late".into(),
                stream: ToolOutputStream::Stdout,
                chunk: "late output must not render".into(),
            })
            .unwrap();

        assert!(!frontend.output().stderr_str().contains("late output"));
    }

    #[test]
    fn two_different_tools_both_render_completion() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        for (id, name) in [("tc_a", "read"), ("tc_b", "bash")] {
            frontend
                .handle_event(&AgentEvent::ToolStarted {
                    run_id: RunId::new(1),
                    tool_call_id: id.into(),
                    name: name.into(),
                    arguments: serde_json::json!({}),
                })
                .unwrap();
            frontend
                .handle_event(&AgentEvent::ToolFinished {
                    run_id: RunId::new(1),
                    tool_call_id: id.into(),
                    name: name.into(),
                    result: AgentToolResult::default(),
                })
                .unwrap();
        }

        assert_eq!(frontend.output().stderr_str().matches("✅").count(), 2);
        assert!(frontend.output().stderr_str().contains("read"));
        assert!(frontend.output().stderr_str().contains("bash"));
    }

    #[test]
    fn streamed_error_content_is_not_repeated_in_completion() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolStarted {
                run_id: RunId::new(1),
                tool_call_id: "tc_error".into(),
                name: "bash".into(),
                arguments: serde_json::json!({}),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "tc_error".into(),
                stream: ToolOutputStream::Stderr,
                chunk: "streamed failure details".into(),
            })
            .unwrap();
        frontend
            .handle_event(&AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: "tc_error".into(),
                name: "bash".into(),
                result: AgentToolResult {
                    content: vec![Content::Text {
                        text: "streamed failure details".into(),
                    }],
                    is_error: true,
                    ..Default::default()
                },
            })
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("streamed failure details").count(), 1);
        assert!(stderr.contains("❌ bash error"));
    }

    #[test]
    fn streamed_timeout_and_abort_render_status_without_repeating_content() {
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        frontend
            .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
            .unwrap();
        for (id, content, timed_out, aborted) in [
            ("tc_timeout", "timeout details", true, false),
            ("tc_abort", "abort details", false, true),
        ] {
            frontend
                .handle_event(&AgentEvent::ToolStarted {
                    run_id: RunId::new(1),
                    tool_call_id: id.into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({}),
                })
                .unwrap();
            frontend
                .handle_event(&AgentEvent::ToolOutput {
                    run_id: RunId::new(1),
                    tool_call_id: id.into(),
                    stream: ToolOutputStream::Stderr,
                    chunk: format!("{content}\n"),
                })
                .unwrap();
            frontend
                .handle_event(&AgentEvent::ToolFinished {
                    run_id: RunId::new(1),
                    tool_call_id: id.into(),
                    name: "bash".into(),
                    result: AgentToolResult {
                        content: vec![Content::Text { text: content.into() }],
                        is_error: true,
                        timed_out,
                        aborted,
                        ..Default::default()
                    },
                })
                .unwrap();
        }

        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("timeout details").count(), 1);
        assert_eq!(stderr.matches("abort details").count(), 1);
        assert_eq!(stderr.matches("timed out").count(), 1);
        assert_eq!(stderr.matches("aborted").count(), 1);
    }

    #[test]
    fn tool_finished_summary_truncates_unicode_safely() {
        let cases = [
            "这是一个很长的中文工具输出，用来验证摘要不会在 UTF-8 字节中间截断".repeat(4),
            "😀😃😄😁😆😅😂🤣😊😇".repeat(10),
            "ASCII 混合文本 😀 with multibyte characters".repeat(3),
        ];
        for text in cases {
            let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
            frontend
                .handle_event(&AgentEvent::RunStarted { run_id: RunId::new(1) })
                .unwrap();
            frontend
                .handle_event(&AgentEvent::ToolFinished {
                    run_id: RunId::new(1),
                    tool_call_id: "tc_unicode".into(),
                    name: "read".into(),
                    result: AgentToolResult {
                        content: vec![Content::Text { text }],
                        ..Default::default()
                    },
                })
                .unwrap();
            let stderr = frontend.output().stderr_str();
            let summary = stderr.lines().last().unwrap_or_default();
            assert!(summary.chars().count() <= 80 + "  ✅ read: ".chars().count());
            assert!(std::str::from_utf8(&frontend.output().stderr).is_ok());
        }
    }

    #[test]
    fn truncate_chars_handles_boundaries_and_multibyte_text() {
        assert_eq!(truncate_chars("short", 80).as_ref(), "short");
        assert_eq!(truncate_chars("exact", 5).as_ref(), "exact");
        assert_eq!(truncate_chars("abcdef", 6).as_ref(), "abcdef");
        assert_eq!(truncate_chars("abcdefg", 6).as_ref(), "abc...");
        assert_eq!(truncate_chars(&"界".repeat(100), 80).chars().count(), 80);
        assert_eq!(truncate_chars(&"😀".repeat(100), 80).chars().count(), 80);
        assert!(truncate_chars(&"😀".repeat(100), 80).ends_with("..."));
    }

    // ── Integration tests: MockProvider → Agent → AgentEvent → drive_print_run → MemoryOutput ──

    #[tokio::test]
    async fn integration_plain_response() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::MockProvider;

        let mock = MockProvider::text("Hello from integration test");
        let mut agent = Agent::new(
            Box::new(mock),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        let token = CancellationToken::new();

        let outcome = drive_print_run(&mut agent, "hi", &mut frontend, token).await;

        assert!(matches!(outcome, Ok(PrintRunOutcome::Finished(_))));
        assert_eq!(frontend.output().stdout_str().trim_end(), "Hello from integration test");
        assert!(frontend.output().stderr.is_empty());
    }

    #[tokio::test]
    async fn integration_tool_call_and_output() {
        use crate::agent::engine::Agent;
        use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionContext};
        use crate::ai::mock::{MockProvider, MockStep};
        use crate::ai::types::{Content, Tool};
        use async_trait::async_trait;

        struct EchoTool;
        impl Tool for EchoTool {
            fn name(&self) -> &str {
                "echo"
            }
            fn description(&self) -> &str {
                "Echoes"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"]})
            }
        }
        #[async_trait]
        impl AgentTool for EchoTool {
            fn label(&self) -> &str {
                "echo"
            }
            async fn execute(
                &self,
                _id: &str,
                params: serde_json::Value,
                ctx: ToolExecutionContext,
            ) -> anyhow::Result<AgentToolResult> {
                let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let _ = ctx
                    .output_tx
                    .send(crate::agent::types::ToolOutputEvent {
                        stream: ToolOutputStream::Stdout,
                        chunk: format!("echo: {text}\n"),
                    })
                    .await;
                Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: format!("echo: {}", text),
                    }],
                    ..Default::default()
                })
            }
        }

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "hello"}),
                stop_reason: None,
            },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(
            Box::new(mock),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        agent.add_tool(Box::new(EchoTool));
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        let token = CancellationToken::new();

        let outcome = drive_print_run(&mut agent, "echo hello", &mut frontend, token).await;

        assert!(matches!(outcome, Ok(PrintRunOutcome::Finished(_))));
        // TextDelta goes to stdout
        assert!(frontend.output().stdout_str().contains("Done"));
        // ToolStarted goes to stderr
        assert!(frontend.output().stderr_str().contains("echo"));
        // ToolFinished summary in stderr (the tool emitted streaming output)
        assert!(frontend.output().stderr_str().contains("done"));
    }

    #[tokio::test]
    async fn integration_bash_zero_exit_code_is_success() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::{MockProvider, MockStep};
        use std::sync::{Arc, RwLock};

        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_bash_success".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "printf '\\163\\165\\143\\143\\145\\163\\163'"}),
                stop_reason: None,
            },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(
            Box::new(mock),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let cwd = Arc::new(RwLock::new(std::env::current_dir().unwrap()));
        agent.add_tool(Box::new(crate::coding_agent::tools::bash::BashTool::new(cwd)));
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());

        let outcome = drive_print_run(
            &mut agent,
            "run a successful bash command",
            &mut frontend,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, PrintRunOutcome::Finished(StopReason::Stop)));
        assert_eq!(frontend.output().stderr_str().matches("success").count(), 1);
        assert!(frontend.output().stderr_str().contains("✅ bash done"));
        assert!(!frontend.output().stderr_str().contains("❌ bash exit code 0"));
        assert!(frontend.output().stdout_str().contains("Done"));
        assert!(!frontend.output().stdout_str().contains("success"));
    }

    #[tokio::test]
    async fn integration_provider_error() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::{MockProvider, MockStep};

        let mock = MockProvider::new(vec![MockStep::Error("API error".into())]);
        let mut agent = Agent::new(
            Box::new(mock),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        let token = CancellationToken::new();

        let outcome = drive_print_run(&mut agent, "trigger error", &mut frontend, token).await;

        assert!(matches!(outcome, Ok(PrintRunOutcome::Finished(_))));
        assert!(frontend.output().stderr_str().contains("Provider error"));
        assert!(frontend.output().stderr_str().contains("API error"));
    }

    #[tokio::test]
    async fn output_failure_cancels_and_settles_agent_before_returning_error() {
        use crate::agent::engine::Agent;
        use crate::ai::providers::{Model, ProviderApi, ProviderRequestContext, ProviderStream};
        use crate::ai::stream::StreamEvent;
        use crate::ai::types::{AgentMessage, Tool};
        use async_trait::async_trait;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct CleanupProvider {
            settled: Arc<AtomicBool>,
        }

        #[async_trait]
        impl ProviderApi for CleanupProvider {
            async fn stream(
                &self,
                _model: &Model,
                _messages: &[AgentMessage],
                _tools: &[&dyn Tool],
                _system_prompt: Option<&str>,
                context: ProviderRequestContext,
            ) -> anyhow::Result<ProviderStream> {
                let (tx, rx) = tokio::sync::mpsc::channel(1);
                let cancellation = context.cancellation.child_token();
                let worker_cancellation = cancellation.clone();
                let settled = self.settled.clone();
                let worker = tokio::spawn(async move {
                    let _ = tx
                        .send(StreamEvent::TextDelta {
                            delta: "this write fails".into(),
                        })
                        .await;
                    worker_cancellation.cancelled().await;
                    settled.store(true, Ordering::SeqCst);
                });
                Ok(ProviderStream::new(rx, worker, cancellation))
            }
        }

        let settled = Arc::new(AtomicBool::new(false));
        let mut agent = Agent::new(
            Box::new(CleanupProvider {
                settled: settled.clone(),
            }),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let token = CancellationToken::new();
        let mut frontend = PrintFrontend::with_output(FailingOutput);

        let error = drive_print_run(&mut agent, "fail output", &mut frontend, token.clone())
            .await
            .expect_err("frontend I/O failure must be returned");
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(token.is_cancelled());
        assert!(
            settled.load(Ordering::SeqCst),
            "provider producer must settle before return"
        );
    }

    #[tokio::test]
    async fn integration_cancellation() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::{MockProvider, MockStep};

        let mock = MockProvider::new(vec![MockStep::Text("never".into())]);
        let mut agent = Agent::new(
            Box::new(mock),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());
        let token = CancellationToken::new();

        // Pre-cancel to simulate immediate cancellation
        token.cancel();

        let outcome = drive_print_run(&mut agent, "cancel me", &mut frontend, token).await;

        assert!(matches!(outcome, Ok(PrintRunOutcome::Aborted)));
        assert!(frontend.output().stderr_str().contains("aborted"));
    }

    #[tokio::test]
    async fn integration_sequential_runs_state_isolation() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::MockProvider;

        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());

        // First run
        let mut agent1 = Agent::new(
            Box::new(MockProvider::text("first response")),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let token1 = CancellationToken::new();
        let outcome1 = drive_print_run(&mut agent1, "first", &mut frontend, token1).await;
        assert!(matches!(outcome1, Ok(PrintRunOutcome::Finished(_))));
        let stdout1 = frontend.output().stdout_str();
        assert!(stdout1.contains("first response"));

        // Second run — frontend state should be reset by RunStarted
        frontend.output_mut().clear();
        let mut agent2 = Agent::new(
            Box::new(MockProvider::text("second response")),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let token2 = CancellationToken::new();
        let outcome2 = drive_print_run(&mut agent2, "second", &mut frontend, token2).await;
        assert!(matches!(outcome2, Ok(PrintRunOutcome::Finished(_))));
        let stdout2 = frontend.output().stdout_str();
        assert!(stdout2.contains("second response"));
        // First response must not leak into second run's output
        assert!(!stdout2.contains("first response"));
    }

    #[tokio::test]
    async fn integration_cancel_then_succeed() {
        use crate::agent::engine::Agent;
        use crate::ai::mock::MockProvider;

        let mut agent1 = Agent::new(
            Box::new(MockProvider::text("never")),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let mut frontend = PrintFrontend::with_output(MemoryOutput::new());

        // Cancel first run
        let token1 = CancellationToken::new();
        token1.cancel();
        let outcome1 = drive_print_run(&mut agent1, "cancel me", &mut frontend, token1).await;
        assert!(matches!(outcome1, Ok(PrintRunOutcome::Aborted)));

        // Second run should succeed
        let mut agent2 = Agent::new(
            Box::new(MockProvider::text("recovered")),
            crate::ai::providers::Model {
                id: "mock",
                api: "mock",
            },
        );
        let token2 = CancellationToken::new();
        let outcome2 = drive_print_run(&mut agent2, "go", &mut frontend, token2).await;
        assert!(matches!(outcome2, Ok(PrintRunOutcome::Finished(_))));
        assert!(frontend.output().stdout_str().contains("recovered"));
    }
}
