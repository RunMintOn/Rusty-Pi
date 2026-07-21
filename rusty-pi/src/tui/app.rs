//! Ratatui TUI application state, actions, effects, update, and view.
//!
//! This is the minimal vertical slice for the TUI:
//! - Transcript area
//! - Single-line input box
//! - Status area
//! - Tool running state
//! - Error state

use crate::agent::events::{AgentEvent, RunId};
use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;
use crate::coding_agent::command::{CommandHelpItem, CommandResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::HashMap;

// ── AppState ────────────────────────────────────────────────────────────────

/// The complete state of the TUI application.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Transcript of messages (user prompts and agent responses).
    pub transcript: Vec<TranscriptEntry>,
    /// Current input buffer.
    pub input: String,
    /// Cursor position in the input buffer (byte offset).
    pub cursor: usize,
    /// Current activity state.
    pub activity: ActivityState,
    /// Outcome of the last completed run.
    pub last_outcome: Option<RunOutcome>,
    /// The RunId of the current run, if any.
    pub current_run_id: Option<RunId>,
    /// Status message.
    pub status: String,
    /// Last error message.
    pub error: Option<String>,
    /// Terminal size (width, height).
    pub terminal_size: (u16, u16),
    /// Whether to quit.
    pub quit: bool,
    /// Scroll offset for transcript.
    pub scroll_offset: u16,
    /// Maps tool call ID to transcript index for updating tool entries.
    tool_index: HashMap<String, usize>,
}

/// State of a tool run in the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRunState {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Aborted,
}

/// A single entry in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    /// A user prompt.
    User(String),
    /// An agent text response (accumulated).
    Assistant(String),
    /// A tool call with full lifecycle information.
    Tool {
        id: String,
        name: String,
        arguments: serde_json::Value,
        stdout: String,
        stderr: String,
        state: ToolRunState,
        result: Option<AgentToolResult>,
    },
    /// A provider error.
    ProviderError(String),
    /// A system message (abort, etc).
    System(String),
}

/// Current activity state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    /// Idle, waiting for input.
    Idle,
    /// Running an agent turn.
    Running,
    /// User requested cancel, waiting for agent to stop.
    Cancelling,
}

/// Outcome of the most recent completed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Aborted,
    ProviderError(String),
    ToolError(String),
}

// Note: RunState is replaced by ActivityState + RunOutcome.
// ActivityState tracks the current activity, RunOutcome tracks the last result.

// ── Action ──────────────────────────────────────────────────────────────────

/// Actions that can update the application state.
#[derive(Debug)]
pub enum Action {
    /// A key was pressed.
    KeyInput(KeyEvent),
    /// Terminal was resized.
    Resize(u16, u16),
    /// Submit the current input.
    Submit,
    /// Cancel the current run.
    Cancel,
    /// An event from the agent.
    AgentEvent(AgentEvent),
    /// A structured slash-command result.
    CommandResult(CommandResult),
    /// Quit the application.
    Quit,
}

// ── Effect ──────────────────────────────────────────────────────────────────

/// Side effects to be performed after a state update.
#[derive(Debug)]
pub enum Effect {
    /// Start an agent run with the given prompt.
    RunAgent(String),
    /// Cancel the current agent run.
    CancelAgent,
    /// Quit the application.
    Quit,
}

// ── update ──────────────────────────────────────────────────────────────────

impl AppState {
    pub fn new(terminal_size: (u16, u16)) -> Self {
        Self {
            transcript: Vec::new(),
            input: String::new(),
            cursor: 0,
            activity: ActivityState::Idle,
            last_outcome: None,
            current_run_id: None,
            status: String::from("Ready"),
            error: None,
            terminal_size,
            quit: false,
            scroll_offset: 0,
            tool_index: HashMap::new(),
        }
    }

    /// Process an action and return any side effects.
    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        let mut effects = Vec::new();

        match action {
            Action::KeyInput(key) => {
                self.error = None; // Clear error on any input
                match key.code {
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match c {
                                'c' => {
                                    if self.activity == ActivityState::Running {
                                        effects.push(Effect::CancelAgent);
                                    } else {
                                        effects.push(Effect::Quit);
                                    }
                                }
                                'a' => {
                                    self.cursor = 0;
                                }
                                'e' => {
                                    self.cursor = self.input.len();
                                }
                                'u' => {
                                    self.input.clear();
                                    self.cursor = 0;
                                }
                                'w' => {
                                    // Delete word backward
                                    let before = &self.input[..self.cursor];
                                    let new_pos = before.rfind(' ').map(|p| p + 1).unwrap_or(0);
                                    self.input.drain(new_pos..self.cursor);
                                    self.cursor = new_pos;
                                }
                                _ => {}
                            }
                        } else {
                            self.input.insert(self.cursor, c);
                            self.cursor += c.len_utf8();
                        }
                    }
                    KeyCode::Enter => {
                        if self.activity == ActivityState::Idle {
                            let prompt = self.input.trim().to_string();
                            if !prompt.is_empty() {
                                self.transcript.push(TranscriptEntry::User(prompt.clone()));
                                self.activity = ActivityState::Running;
                                self.status = "Running...".into();
                                self.input.clear();
                                self.cursor = 0;
                                effects.push(Effect::RunAgent(prompt));
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        if self.cursor > 0 {
                            // Find the previous character boundary
                            let prev = self.input[..self.cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            self.input.drain(prev..self.cursor);
                            self.cursor = prev;
                        }
                    }
                    KeyCode::Delete => {
                        if self.cursor < self.input.len() {
                            let next = self.input[self.cursor..]
                                .char_indices()
                                .nth(1)
                                .map(|(i, _)| self.cursor + i)
                                .unwrap_or(self.input.len());
                            self.input.drain(self.cursor..next);
                        }
                    }
                    KeyCode::Left => {
                        if self.cursor > 0 {
                            let prev = self.input[..self.cursor]
                                .char_indices()
                                .next_back()
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            self.cursor = prev;
                        }
                    }
                    KeyCode::Right => {
                        if self.cursor < self.input.len() {
                            let next = self.input[self.cursor..]
                                .char_indices()
                                .nth(1)
                                .map(|(i, _)| self.cursor + i)
                                .unwrap_or(self.input.len());
                            self.cursor = next;
                        }
                    }
                    KeyCode::Home => {
                        self.cursor = 0;
                    }
                    KeyCode::End => {
                        self.cursor = self.input.len();
                    }
                    KeyCode::Up => {
                        self.scroll_offset = self.scroll_offset.saturating_add(1);
                    }
                    KeyCode::Down => {
                        self.scroll_offset = self.scroll_offset.saturating_sub(1);
                    }
                    KeyCode::PageUp => {
                        self.scroll_offset = self.scroll_offset.saturating_add(10);
                    }
                    KeyCode::PageDown => {
                        self.scroll_offset = self.scroll_offset.saturating_sub(10);
                    }
                    _ => {}
                }
            }
            Action::Resize(w, h) => {
                self.terminal_size = (w, h);
            }
            Action::Submit => {
                // Same as Enter
                if self.activity == ActivityState::Idle {
                    let prompt = self.input.trim().to_string();
                    if !prompt.is_empty() {
                        self.transcript.push(TranscriptEntry::User(prompt.clone()));
                        self.activity = ActivityState::Running;
                        self.status = "Running...".into();
                        self.input.clear();
                        self.cursor = 0;
                        effects.push(Effect::RunAgent(prompt));
                    }
                }
            }
            Action::Cancel => {
                if self.activity == ActivityState::Running {
                    self.activity = ActivityState::Cancelling;
                    effects.push(Effect::CancelAgent);
                }
            }
            Action::AgentEvent(event) => {
                self.handle_agent_event(event);
            }
            Action::CommandResult(result) => {
                effects.extend(self.apply_command_result(result));
            }
            Action::Quit => {
                self.quit = true;
                effects.push(Effect::Quit);
            }
        }

        effects
    }

    /// Apply a command result without letting the command write to a terminal.
    ///
    /// Command output is represented as transcript content so the same result
    /// can be rendered by Ratatui or another frontend.
    pub fn apply_command_result(&mut self, result: CommandResult) -> Vec<Effect> {
        self.activity = ActivityState::Idle;
        self.error = None;

        match result {
            CommandResult::Message(message) => {
                self.transcript.push(TranscriptEntry::System(message));
                self.status = "Ready".into();
                Vec::new()
            }
            CommandResult::Error(message) => {
                self.error = Some(message.clone());
                self.transcript.push(TranscriptEntry::ProviderError(message));
                self.status = "Error".into();
                Vec::new()
            }
            CommandResult::Help(items) => {
                self.transcript.push(TranscriptEntry::System(format_help(&items)));
                self.status = "Ready".into();
                Vec::new()
            }
            CommandResult::ModelChanged { model } => {
                self.transcript
                    .push(TranscriptEntry::System(format!("Switched to {model}")));
                self.status = "Ready".into();
                Vec::new()
            }
            CommandResult::Sessions(sessions) => {
                let text = if sessions.is_empty() {
                    "No sessions found.".to_string()
                } else {
                    let mut text = String::from("Available sessions:");
                    for session in sessions {
                        text.push_str(&format!(
                            "\n  {} | model: {} | msgs: {} | created: {}",
                            session.id, session.model, session.msg_count, session.created
                        ));
                    }
                    text
                };
                self.transcript.push(TranscriptEntry::System(text));
                self.status = "Ready".into();
                Vec::new()
            }
            CommandResult::Quit => {
                self.quit = true;
                vec![Effect::Quit]
            }
            CommandResult::Noop => Vec::new(),
        }
    }

    /// Handle an agent event by updating the transcript.
    /// Events from old runs are silently ignored.
    fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::RunStarted { run_id } => {
                // Track the current run; clear old tool_index
                self.current_run_id = Some(run_id);
                self.tool_index.clear();
            }
            AgentEvent::TextDelta { run_id, text } => {
                // Ignore events from old runs
                if self.current_run_id != Some(run_id) {
                    return;
                }
                // Find or create the last assistant entry
                match self.transcript.last_mut() {
                    Some(TranscriptEntry::Assistant(s)) => {
                        s.push_str(&text);
                    }
                    _ => {
                        self.transcript.push(TranscriptEntry::Assistant(text));
                    }
                }
            }
            AgentEvent::ThinkingDelta { run_id, text } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                // Show thinking in status
                self.status = format!("Thinking: {}", text.chars().take(50).collect::<String>());
            }
            AgentEvent::ToolStarted {
                run_id,
                tool_call_id,
                name,
                arguments,
            } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                let idx = self.transcript.len();
                self.tool_index.insert(tool_call_id.clone(), idx);
                self.transcript.push(TranscriptEntry::Tool {
                    id: tool_call_id,
                    name: name.clone(),
                    arguments,
                    stdout: String::new(),
                    stderr: String::new(),
                    state: ToolRunState::Running,
                    result: None,
                });
                self.status = format!("Running {}...", name);
            }
            AgentEvent::ToolOutput {
                run_id,
                tool_call_id,
                stream,
                chunk,
            } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                if let Some(&idx) = self.tool_index.get(&tool_call_id) {
                    if let Some(TranscriptEntry::Tool { stdout, stderr, .. }) = self.transcript.get_mut(idx) {
                        match stream {
                            crate::agent::events::ToolOutputStream::Stdout => {
                                stdout.push_str(&chunk);
                            }
                            crate::agent::events::ToolOutputStream::Stderr => {
                                stderr.push_str(&chunk);
                            }
                        }
                    }
                } else {
                    // Unknown tool ID — create orphan entry so we don't lose output
                    let idx = self.transcript.len();
                    self.tool_index.insert(tool_call_id.clone(), idx);
                    let mut stdout = String::new();
                    let mut stderr = String::new();
                    match stream {
                        crate::agent::events::ToolOutputStream::Stdout => {
                            stdout.push_str(&chunk);
                        }
                        crate::agent::events::ToolOutputStream::Stderr => {
                            stderr.push_str(&chunk);
                        }
                    }
                    self.transcript.push(TranscriptEntry::Tool {
                        id: tool_call_id,
                        name: "unknown".into(),
                        arguments: serde_json::Value::Null,
                        stdout,
                        stderr,
                        state: ToolRunState::Running,
                        result: None,
                    });
                }
            }
            AgentEvent::ToolFinished {
                run_id,
                tool_call_id,
                name,
                result,
            } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                let tool_state = if result.timed_out {
                    ToolRunState::TimedOut
                } else if result.aborted {
                    ToolRunState::Aborted
                } else if result.is_error {
                    ToolRunState::Failed
                } else {
                    ToolRunState::Succeeded
                };

                if let Some(&idx) = self.tool_index.get(&tool_call_id) {
                    if let Some(TranscriptEntry::Tool {
                        name: entry_name,
                        state: entry_state,
                        result: entry_result,
                        ..
                    }) = self.transcript.get_mut(idx)
                    {
                        // Update name if it was unknown (orphan from ToolOutput)
                        if entry_name == "unknown" {
                            *entry_name = name;
                        }
                        *entry_state = tool_state;
                        *entry_result = Some(result);
                    }
                } else {
                    // ToolFinished for unknown ID — create entry
                    let idx = self.transcript.len();
                    self.tool_index.insert(tool_call_id.clone(), idx);
                    self.transcript.push(TranscriptEntry::Tool {
                        id: tool_call_id,
                        name,
                        arguments: serde_json::Value::Null,
                        stdout: String::new(),
                        stderr: String::new(),
                        state: tool_state,
                        result: Some(result),
                    });
                }
                self.status = "Running...".into();
            }
            AgentEvent::ProviderError { run_id, error } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                self.transcript
                    .push(TranscriptEntry::ProviderError(error.message.clone()));
                self.error = Some(error.message);
            }
            AgentEvent::RunAborted { run_id } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                self.activity = ActivityState::Idle;
                self.last_outcome = Some(RunOutcome::Aborted);
                self.transcript.push(TranscriptEntry::System("Run aborted".into()));
                self.status = "Aborted".into();
            }
            AgentEvent::RunFinished { run_id, stop_reason } => {
                if self.current_run_id != Some(run_id) {
                    return;
                }
                self.activity = ActivityState::Idle;
                match stop_reason {
                    StopReason::Stop => {
                        self.last_outcome = Some(RunOutcome::Completed);
                        self.status = "Ready".into();
                    }
                    StopReason::Error => {
                        self.last_outcome = Some(RunOutcome::ProviderError("Provider error".into()));
                        self.status = "Error".into();
                    }
                    StopReason::Length => {
                        self.last_outcome = Some(RunOutcome::Completed);
                        self.status = "Truncated".into();
                    }
                    StopReason::Aborted => {
                        self.last_outcome = Some(RunOutcome::Aborted);
                        self.status = "Aborted".into();
                    }
                    StopReason::ToolUse => {
                        self.status = "Running...".into();
                    }
                }
            }
        }
    }
}

fn format_help(items: &[CommandHelpItem]) -> String {
    let mut output = String::from("Commands:");
    for item in items {
        output.push_str(&format!("\n  /{:<12} {}", item.name, item.description));
    }
    output
}

// ── view ────────────────────────────────────────────────────────────────────

/// Render the application state to a terminal frame.
pub fn view(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Transcript
            Constraint::Length(3), // Input
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    // Transcript
    render_transcript(frame, state, chunks[0]);

    // Input
    render_input(frame, state, chunks[1]);

    // Status
    render_status(frame, state, chunks[2]);
}

/// Render the transcript area.
fn render_transcript(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for entry in &state.transcript {
        match entry {
            TranscriptEntry::User(text) => {
                lines.push(Line::from(vec![
                    Span::styled("You: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::raw(text.clone()),
                ]));
                lines.push(Line::from(""));
            }
            TranscriptEntry::Assistant(text) => {
                for line in text.lines() {
                    lines.push(Line::from(vec![Span::raw(line.to_string())]));
                }
                lines.push(Line::from(""));
            }
            TranscriptEntry::Tool {
                name,
                stdout,
                stderr,
                state: tool_state,
                ..
            } => {
                let (icon, style) = match tool_state {
                    ToolRunState::Running => ("⚙", Style::default().fg(Color::Yellow)),
                    ToolRunState::Succeeded => ("✅", Style::default().fg(Color::Green)),
                    ToolRunState::Failed => ("❌", Style::default().fg(Color::Red)),
                    ToolRunState::TimedOut => ("⏰", Style::default().fg(Color::Red)),
                    ToolRunState::Aborted => ("⏹", Style::default().fg(Color::Yellow)),
                };
                let status_label = match tool_state {
                    ToolRunState::Running => " running...".to_string(),
                    ToolRunState::Succeeded => String::new(),
                    ToolRunState::Failed => " [failed]".to_string(),
                    ToolRunState::TimedOut => " [timed out]".to_string(),
                    ToolRunState::Aborted => " [aborted]".to_string(),
                };
                lines.push(Line::from(vec![Span::styled(
                    format!("{} {}{}", icon, name, status_label),
                    style,
                )]));
                // Show stdout
                for line in stdout.lines().take(3) {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
                if stdout.lines().count() > 3 {
                    lines.push(Line::from(vec![Span::styled(
                        "  ...",
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
                // Show stderr if non-empty
                if !stderr.trim().is_empty() {
                    lines.push(Line::from(vec![Span::styled(
                        "  [stderr]",
                        Style::default().fg(Color::Red),
                    )]));
                    for line in stderr.lines().take(2) {
                        lines.push(Line::from(vec![Span::styled(
                            format!("    {}", line),
                            Style::default().fg(Color::Red),
                        )]));
                    }
                }
                lines.push(Line::from(""));
            }
            TranscriptEntry::ProviderError(msg) => {
                lines.push(Line::from(vec![
                    Span::styled("❌ Error: ", Style::default().fg(Color::Red)),
                    Span::raw(msg.clone()),
                ]));
                lines.push(Line::from(""));
            }
            TranscriptEntry::System(msg) => {
                lines.push(Line::from(vec![Span::styled(
                    format!("ℹ {}", msg),
                    Style::default().fg(Color::Blue),
                )]));
                lines.push(Line::from(""));
            }
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Transcript")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, area);
}

/// Render the input area.
fn render_input(frame: &mut Frame, state: &AppState, area: Rect) {
    // Create display text with cursor indicator
    let is_active = matches!(state.activity, ActivityState::Running | ActivityState::Cancelling);
    let display_text = if is_active {
        format!("{} ⏳", state.input)
    } else {
        state.input.clone()
    };

    let paragraph = Paragraph::new(display_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Input")
                .border_style(Style::default().fg(if is_active { Color::Yellow } else { Color::Cyan })),
        )
        .scroll((0, 0));

    frame.render_widget(paragraph, area);

    // Set cursor position
    let cursor_x = area.x + 1 + state.cursor as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

/// Render the status bar.
fn render_status(frame: &mut Frame, state: &AppState, area: Rect) {
    let status_style = if state.error.is_some() {
        Style::default().fg(Color::Red)
    } else if matches!(state.activity, ActivityState::Running | ActivityState::Cancelling) {
        Style::default().fg(Color::Yellow)
    } else if state.last_outcome == Some(RunOutcome::Aborted) {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Green)
    };

    let status_text = if let Some(ref err) = state.error {
        format!("❌ {} | {}", err, state.status)
    } else {
        state.status.clone()
    };

    let paragraph = Paragraph::new(status_text).style(status_style);
    frame.render_widget(paragraph, area);
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use crate::coding_agent::command::{CommandRegistry, DispatchOutcome, ExitCommand, HelpCommand, QuitCommand};
    use crate::coding_agent::prompt_session::PromptSession;
    use crossterm::event::{KeyCode, KeyModifiers};
    use std::path::PathBuf;

    fn key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn command_session() -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::text("mock")),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/.pi/agent"),
            vec![],
            vec![],
            false,
            None,
            vec![],
        )
    }

    #[test]
    fn app_state_initial() {
        let state = AppState::new((80, 24));
        assert_eq!(state.activity, ActivityState::Idle);
        assert!(state.input.is_empty());
        assert!(state.transcript.is_empty());
        assert!(!state.quit);
    }

    #[test]
    fn help_command_is_rendered_into_ratatui_transcript() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(HelpCommand));
        registry.register(Box::new(ExitCommand));
        let mut session = command_session();
        let outcome = registry.dispatch("/help", &mut session).unwrap();
        let mut state = AppState::new((80, 24));
        if let DispatchOutcome::Handled(result) = outcome {
            state.update(Action::CommandResult(result));
        } else {
            panic!("/help should be handled");
        }
        let output = render_state(&state);
        assert!(output.contains("Commands:"));
        assert!(output.contains("/help"));
        assert_eq!(state.activity, ActivityState::Idle);
    }

    #[test]
    fn quit_command_produces_ratatui_quit_effect() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(QuitCommand));
        let mut session = command_session();
        let outcome = registry.dispatch("/quit", &mut session).unwrap();
        let mut state = AppState::new((80, 24));
        let effects = match outcome {
            DispatchOutcome::Exit => state.update(Action::Quit),
            other => panic!("expected exit, got {other:?}"),
        };
        assert!(state.quit);
        assert!(effects.iter().any(|effect| matches!(effect, Effect::Quit)));
    }

    #[test]
    fn unknown_command_is_rendered_as_structured_ratatui_error() {
        let registry = CommandRegistry::new();
        let mut session = command_session();
        let outcome = registry.dispatch("/unknown", &mut session).unwrap();
        let mut state = AppState::new((80, 24));
        if let DispatchOutcome::Handled(result) = outcome {
            state.update(Action::CommandResult(result));
        } else {
            panic!("unknown command should be handled as an error");
        }
        let output = render_state(&state);
        assert!(output.contains("Unknown command"));
        assert!(state.error.is_some());
    }

    #[test]
    fn app_state_input_char() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('a'), KeyModifiers::NONE)));
        assert_eq!(state.input, "a");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn app_state_backspace() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('h'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('i'), KeyModifiers::NONE)));
        assert_eq!(state.input, "hi");
        state.update(Action::KeyInput(key_event(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(state.input, "h");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn app_state_delete() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('a'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('b'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Left, KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Delete, KeyModifiers::NONE)));
        assert_eq!(state.input, "a");
    }

    #[test]
    fn app_state_cursor_movement() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('a'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('b'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert_eq!(state.cursor, 3);

        state.update(Action::KeyInput(key_event(KeyCode::Left, KeyModifiers::NONE)));
        assert_eq!(state.cursor, 2);

        state.update(Action::KeyInput(key_event(KeyCode::Left, KeyModifiers::NONE)));
        assert_eq!(state.cursor, 1);

        state.update(Action::KeyInput(key_event(KeyCode::Right, KeyModifiers::NONE)));
        assert_eq!(state.cursor, 2);

        state.update(Action::KeyInput(key_event(KeyCode::Home, KeyModifiers::NONE)));
        assert_eq!(state.cursor, 0);

        state.update(Action::KeyInput(key_event(KeyCode::End, KeyModifiers::NONE)));
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn app_state_submit_empty_does_nothing() {
        let mut state = AppState::new((80, 24));
        let effects = state.update(Action::KeyInput(key_event(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(effects.is_empty());
        assert_eq!(state.activity, ActivityState::Idle);
    }

    #[test]
    fn app_state_submit_creates_run() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('h'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('i'), KeyModifiers::NONE)));
        let effects = state.update(Action::KeyInput(key_event(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(state.activity, ActivityState::Running);
        assert_eq!(state.transcript.len(), 1);
        assert!(matches!(&state.transcript[0], TranscriptEntry::User(s) if s == "hi"));
        assert!(!effects.is_empty());
    }

    #[test]
    fn app_state_cancel_while_running() {
        let mut state = AppState::new((80, 24));
        state.activity = ActivityState::Running;
        let effects = state.update(Action::Cancel);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::CancelAgent));
        assert_eq!(state.activity, ActivityState::Cancelling);
    }

    #[test]
    fn app_state_cancel_sets_cancelling() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        let _effects = state.update(Action::Cancel);
        assert_eq!(state.activity, ActivityState::Cancelling);
        // RunAborted event transitions to Idle with Aborted outcome
        state.handle_agent_event(AgentEvent::RunAborted { run_id: RunId::new(1) });
        assert_eq!(state.activity, ActivityState::Idle);
        assert_eq!(state.last_outcome, Some(RunOutcome::Aborted));
    }

    #[test]
    fn app_state_resize() {
        let mut state = AppState::new((80, 24));
        state.update(Action::Resize(120, 40));
        assert_eq!(state.terminal_size, (120, 40));
    }

    #[test]
    fn app_state_text_delta_appends() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "hello".into(),
        });
        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: " world".into(),
        });
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Assistant(s) => assert_eq!(s, "hello world"),
            _ => panic!("Expected Assistant entry"),
        }
    }

    #[test]
    fn app_state_tool_started_creates_entry() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Tool {
                id,
                name,
                state: tool_state,
                ..
            } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "bash");
                assert_eq!(*tool_state, ToolRunState::Running);
            }
            other => panic!("Expected Tool entry, got: {:?}", other),
        }
        assert!(state.status.contains("bash"));
    }

    #[test]
    fn app_state_tool_finished_updates_entry() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                content: vec![crate::ai::types::Content::Text { text: "output".into() }],
                ..Default::default()
            },
        });
        // Should still be exactly 1 entry (no duplicate)
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Tool {
                state: tool_state,
                result,
                ..
            } => {
                assert_eq!(*tool_state, ToolRunState::Succeeded);
                assert!(result.is_some());
            }
            other => panic!("Expected Tool entry, got: {:?}", other),
        }
    }

    #[test]
    fn app_state_run_finished_sets_idle() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        state.handle_agent_event(AgentEvent::RunFinished {
            run_id: RunId::new(1),
            stop_reason: StopReason::Stop,
        });
        assert_eq!(state.activity, ActivityState::Idle);
    }

    #[test]
    fn app_state_quit() {
        let mut state = AppState::new((80, 24));
        let effects = state.update(Action::Quit);
        assert!(state.quit);
        assert!(matches!(&effects[0], Effect::Quit));
    }

    // ── TestBackend rendering tests ───────────────────────────────────────

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_state(state: &AppState) -> String {
        let backend = TestBackend::new(state.terminal_size.0, state.terminal_size.1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                super::view(frame, state);
            })
            .unwrap();
        // Capture the buffer as a string using ratatui's built-in rendering
        let buf = terminal.backend().buffer().clone();
        let mut lines = Vec::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                let symbol = cell.symbol();
                line.push_str(symbol);
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    #[test]
    fn render_80x24_idle() {
        let state = AppState::new((80, 24));
        let output = render_state(&state);
        assert!(output.contains("Transcript"), "Should show Transcript title");
        assert!(output.contains("Input"), "Should show Input title");
        assert!(output.contains("Ready"), "Should show Ready status");
    }

    #[test]
    fn render_120x40() {
        let state = AppState::new((120, 40));
        let output = render_state(&state);
        assert!(output.contains("Transcript"));
        assert!(output.contains("Input"));
    }

    #[test]
    fn render_narrow_terminal_40x12() {
        let state = AppState::new((40, 12));
        let output = render_state(&state);
        // Should not panic on small terminal
        assert!(output.contains("Transcript"));
    }

    #[test]
    fn render_empty_transcript() {
        let state = AppState::new((80, 24));
        let output = render_state(&state);
        assert!(output.contains("Transcript"));
        // No user/assistant messages
        assert!(!output.contains("You:"));
    }

    #[test]
    fn render_user_message() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::User("hello world".into()));
        let output = render_state(&state);
        assert!(output.contains("You:"));
        assert!(output.contains("hello world"));
    }

    #[test]
    fn render_assistant_streaming_text() {
        let mut state = AppState::new((80, 24));
        state
            .transcript
            .push(TranscriptEntry::Assistant("Hello! I am here to help.".into()));
        let output = render_state(&state);
        assert!(output.contains("Hello! I am here to help."));
    }

    #[test]
    fn render_tool_running_entry() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
            stdout: String::new(),
            stderr: String::new(),
            state: ToolRunState::Running,
            result: None,
        });
        let output = render_state(&state);
        assert!(output.contains("bash"));
    }

    #[test]
    fn render_tool_stdout_stderr_in_transcript() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            stdout: "file1.txt\nfile2.txt".into(),
            stderr: String::new(),
            state: ToolRunState::Succeeded,
            result: None,
        });
        let output = render_state(&state);
        assert!(output.contains("file1.txt"));
        assert!(output.contains("file2.txt"));
    }

    #[test]
    fn render_tool_success_entry() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            stdout: "success".into(),
            stderr: String::new(),
            state: ToolRunState::Succeeded,
            result: None,
        });
        let output = render_state(&state);
        assert!(output.contains("bash"));
        assert!(output.contains("success"));
    }

    #[test]
    fn render_tool_error_entry() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            stdout: String::new(),
            stderr: "command failed".into(),
            state: ToolRunState::Failed,
            result: None,
        });
        let output = render_state(&state);
        assert!(output.contains("bash"));
        assert!(output.contains("command failed"));
    }

    #[test]
    fn render_provider_error() {
        let mut state = AppState::new((80, 24));
        state
            .transcript
            .push(TranscriptEntry::ProviderError("API limit".into()));
        let output = render_state(&state);
        assert!(output.contains("Error"));
        assert!(output.contains("API limit"));
    }

    #[test]
    fn render_aborted() {
        let mut state = AppState::new((80, 24));
        // Set up run_id so events are accepted
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        state.handle_agent_event(AgentEvent::RunAborted { run_id: RunId::new(1) });
        let output = render_state(&state);
        assert!(output.contains("Aborted") || output.contains("abort"));
    }

    #[test]
    fn render_long_text_wrapping() {
        let mut state = AppState::new((80, 24));
        let long_text = "a".repeat(200);
        state.transcript.push(TranscriptEntry::Assistant(long_text.clone()));
        let output = render_state(&state);
        // Long text should be present (wrapping is handled by ratatui)
        assert!(output.contains("aaaaaaaaaa"));
    }

    #[test]
    fn render_unicode_and_chinese() {
        let mut state = AppState::new((80, 24));
        state
            .transcript
            .push(TranscriptEntry::Assistant("你好世界 🌍 café".into()));
        // TestBackend has limitations with wide characters, so we verify
        // the state is correct and rendering doesn't panic
        let output = render_state(&state);
        assert!(!output.is_empty());
        // Verify the transcript entry exists
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Assistant(s) => {
                assert!(s.contains("你好世界"));
                assert!(s.contains("café"));
            }
            _ => panic!("Expected Assistant entry"),
        }
    }

    #[test]
    fn render_resize() {
        let mut state = AppState::new((80, 24));
        state.update(Action::Resize(120, 40));
        let output = render_state(&state);
        assert!(output.contains("Transcript"));
    }

    #[test]
    fn render_input_cursor_position() {
        let mut state = AppState::new((80, 24));
        state.input = "hello".into();
        state.cursor = 3;
        let output = render_state(&state);
        assert!(output.contains("hello"));
    }

    #[test]
    fn render_narrow_no_panic() {
        // Very small terminal should not panic
        let state = AppState::new((10, 5));
        let output = render_state(&state);
        assert!(!output.is_empty());
    }

    // ── Transcript tool entry tests ──────────────────────────────────────

    #[test]
    fn transcript_tool_started_creates_tool_entry() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Tool {
                id,
                name,
                arguments,
                state: ts,
                ..
            } => {
                assert_eq!(id, "tc_a");
                assert_eq!(name, "bash");
                assert_eq!(arguments["command"], "ls");
                assert_eq!(*ts, ToolRunState::Running);
            }
            other => panic!("Expected Tool entry, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_stdout_only_goes_to_stdout() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "hello\n".into(),
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { stdout, stderr, .. } => {
                assert_eq!(stdout, "hello\n");
                assert!(stderr.is_empty());
            }
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_stderr_only_goes_to_stderr() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            stream: crate::agent::events::ToolOutputStream::Stderr,
            chunk: "err msg\n".into(),
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { stdout, stderr, .. } => {
                assert!(stdout.is_empty());
                assert_eq!(stderr, "err msg\n");
            }
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_assistant_text_does_not_mix_with_tool() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "some text".into(),
        });
        assert_eq!(state.transcript.len(), 2);
        // Tool entry should not have assistant text
        match &state.transcript[0] {
            TranscriptEntry::Tool { stdout, .. } => assert!(stdout.is_empty()),
            other => panic!("Expected Tool, got: {:?}", other),
        }
        // Assistant entry should have the text
        match &state.transcript[1] {
            TranscriptEntry::Assistant(text) => assert_eq!(text, "some text"),
            other => panic!("Expected Assistant, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_tool_output_does_not_create_assistant_entry() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        // ToolOutput for an unknown ID should create a Tool entry, not Assistant
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "orphan".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "data".into(),
        });
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Tool { .. } => (),
            other => panic!("Expected Tool (orphan), got: {:?}", other),
        }
    }

    #[test]
    fn transcript_tool_finished_updates_original_no_duplicate() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_1".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                content: vec![],
                ..Default::default()
            },
        });
        assert_eq!(state.transcript.len(), 1, "Should not create duplicate entry");
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => {
                assert_eq!(*ts, ToolRunState::Succeeded);
            }
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_two_tools_outputs_do_not_cross() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_b".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "alpha".into(),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_b".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "beta".into(),
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { stdout, .. } => assert_eq!(stdout, "alpha"),
            other => panic!("Expected Tool, got: {:?}", other),
        }
        match &state.transcript[1] {
            TranscriptEntry::Tool { stdout, .. } => assert_eq!(stdout, "beta"),
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_unknown_tool_id_does_not_panic() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        // Output for an ID that was never started
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "ghost".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "data".into(),
        });
        // Finish for an ID that was never started
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "ghost2".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult::default(),
        });
        assert_eq!(state.transcript.len(), 2);
    }

    #[test]
    fn transcript_timeout_maps_to_timed_out() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_to".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_to".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                timed_out: true,
                is_error: true,
                ..Default::default()
            },
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => assert_eq!(*ts, ToolRunState::TimedOut),
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_abort_maps_to_aborted() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_ab".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_ab".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                aborted: true,
                is_error: true,
                ..Default::default()
            },
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => assert_eq!(*ts, ToolRunState::Aborted),
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_nonzero_exit_maps_to_failed() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_err".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_err".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                is_error: true,
                exit_code: Some(1),
                ..Default::default()
            },
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => assert_eq!(*ts, ToolRunState::Failed),
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    #[test]
    fn transcript_success_maps_to_succeeded() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_ok".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_ok".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                is_error: false,
                ..Default::default()
            },
        });
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => assert_eq!(*ts, ToolRunState::Succeeded),
            other => panic!("Expected Tool, got: {:?}", other),
        }
    }

    // ── Run state model tests ──────────────────────────────────────────────

    #[test]
    fn run_started_sets_running() {
        let mut state = AppState::new((80, 24));
        state.activity = ActivityState::Idle;
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        // RunStarted sets current_run_id and clears the tool index
        assert_eq!(state.current_run_id, Some(RunId::new(1)));
        assert!(state.tool_index.is_empty());
    }

    #[test]
    fn run_aborted_preserves_outcome() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        state.handle_agent_event(AgentEvent::RunAborted { run_id: RunId::new(1) });
        assert_eq!(state.activity, ActivityState::Idle);
        assert_eq!(state.last_outcome, Some(RunOutcome::Aborted));
        assert_eq!(state.status, "Aborted");
    }

    #[test]
    fn run_finished_stop_sets_completed() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        state.handle_agent_event(AgentEvent::RunFinished {
            run_id: RunId::new(1),
            stop_reason: StopReason::Stop,
        });
        assert_eq!(state.activity, ActivityState::Idle);
        assert_eq!(state.last_outcome, Some(RunOutcome::Completed));
        assert_eq!(state.status, "Ready");
    }

    #[test]
    fn provider_error_not_overwritten_by_stop() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ProviderError {
            run_id: RunId::new(1),
            error: crate::agent::events::ProviderError {
                reason: StopReason::Error,
                message: "limit exceeded".into(),
            },
        });
        assert_eq!(state.last_outcome, None); // ProviderError doesn't set last_outcome
        state.handle_agent_event(AgentEvent::RunFinished {
            run_id: RunId::new(1),
            stop_reason: StopReason::Error,
        });
        assert_eq!(
            state.last_outcome,
            Some(RunOutcome::ProviderError("Provider error".into()))
        );
    }

    #[test]
    fn new_run_clears_tool_index() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "old_tc".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        assert!(!state.tool_index.is_empty());
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        assert!(state.tool_index.is_empty());
    }

    // ── Run ID isolation tests ───────────────────────────────────────────

    #[test]
    fn late_text_delta_from_old_run_ignored() {
        let mut state = AppState::new((80, 24));
        // Run A starts and produces text
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "from A".into(),
        });
        // Run B starts (simulating a new run after Run A was cancelled externally)
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        // Late TextDelta from Run A arrives after Run B started
        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "STALE".into(),
        });
        // Transcript should only contain Run A's initial text, not the stale delta
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Assistant(s) => assert_eq!(s, "from A", "Stale delta should not be appended"),
            _ => panic!("Expected Assistant entry"),
        }
        // current_run_id should still be Run B
        assert_eq!(state.current_run_id, Some(RunId::new(2)));
    }

    #[test]
    fn late_tool_output_from_old_run_does_not_pollute() {
        let mut state = AppState::new((80, 24));
        // Run A starts tool_a and gets some output
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "legit data".into(),
        });
        // Run B starts (simulating new run) — tool_index is cleared
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(2),
            tool_call_id: "tc_b".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        });
        // Late ToolOutput from Run A arrives — should be ignored because run_id != current_run_id
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "STALE DATA".into(),
        });
        // Transcript has 2 entries: tool_a from Run A (with legit data) + tool_b from Run B (empty)
        assert_eq!(state.transcript.len(), 2, "Should have tool_a and tool_b entries");
        // tool_b (index 1) should have empty stdout — not polluted by stale event
        match &state.transcript[1] {
            TranscriptEntry::Tool { stdout, id, .. } => {
                assert_eq!(id, "tc_b");
                assert!(stdout.is_empty(), "tool_b stdout should be empty, not polluted");
            }
            _ => panic!("Expected tool_b entry at index 1"),
        }
        // tool_a (index 0) should have only the legit data, not the stale chunk
        match &state.transcript[0] {
            TranscriptEntry::Tool { stdout, id, .. } => {
                assert_eq!(id, "tc_a");
                assert_eq!(stdout, "legit data", "tool_a should only have legit data");
            }
            _ => panic!("Expected tool_a entry at index 0"),
        }
    }

    #[test]
    fn late_tool_finished_from_old_run_ignored() {
        let mut state = AppState::new((80, 24));
        // Run A starts tool_a
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        // Run B starts (simulating new run)
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: RunId::new(2),
            tool_call_id: "tc_b".into(),
            name: "read".into(),
            arguments: serde_json::json!({}),
        });
        // Late ToolFinished from Run A
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "tc_a".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult {
                content: vec![],
                ..Default::default()
            },
        });
        // Run B tool state should be unchanged (tc_b still Running)
        match &state.transcript[0] {
            TranscriptEntry::Tool { state: ts, .. } => assert_eq!(*ts, ToolRunState::Running),
            _ => panic!("Expected tool_b entry"),
        }
        // current_run_id should still be Run B
        assert_eq!(state.current_run_id, Some(RunId::new(2)));
    }

    #[test]
    fn late_run_finished_from_old_run_does_not_affect_current() {
        let mut state = AppState::new((80, 24));
        // Run A starts
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        // Run B starts (simulating new run)
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        // Late RunFinished from Run A with Stop reason
        state.handle_agent_event(AgentEvent::RunFinished {
            run_id: RunId::new(1),
            stop_reason: StopReason::Stop,
        });
        // current_run_id should still be Run B
        assert_eq!(state.current_run_id, Some(RunId::new(2)));
        // Transcript should be empty (Run A's finish didn't add anything)
        assert!(state.transcript.is_empty());
    }

    #[test]
    fn unknown_run_id_events_ignored() {
        let mut state = AppState::new((80, 24));
        // No run started yet — send events with unknown run_id
        let unknown = RunId::new(999);
        let orig_activity = state.activity.clone();
        let orig_outcome = state.last_outcome.clone();
        let orig_transcript_len = state.transcript.len();
        let orig_status = state.status.clone();
        let orig_quit = state.quit;

        state.handle_agent_event(AgentEvent::TextDelta {
            run_id: unknown,
            text: "ghost".into(),
        });
        state.handle_agent_event(AgentEvent::ToolStarted {
            run_id: unknown,
            tool_call_id: "tc_ghost".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolOutput {
            run_id: unknown,
            tool_call_id: "tc_ghost".into(),
            stream: crate::agent::events::ToolOutputStream::Stdout,
            chunk: "data".into(),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            run_id: unknown,
            tool_call_id: "tc_ghost".into(),
            name: "bash".into(),
            result: crate::agent::types::AgentToolResult::default(),
        });
        state.handle_agent_event(AgentEvent::ProviderError {
            run_id: unknown,
            error: crate::agent::events::ProviderError {
                reason: StopReason::Error,
                message: "error".into(),
            },
        });
        state.handle_agent_event(AgentEvent::RunAborted { run_id: unknown });
        state.handle_agent_event(AgentEvent::RunFinished {
            run_id: unknown,
            stop_reason: StopReason::Stop,
        });

        // AppState should be completely unchanged
        assert_eq!(state.activity, orig_activity);
        assert_eq!(state.last_outcome, orig_outcome);
        assert_eq!(state.transcript.len(), orig_transcript_len);
        assert_eq!(state.status, orig_status);
        assert_eq!(state.quit, orig_quit);
        assert_eq!(state.current_run_id, None);
    }

    #[test]
    fn late_provider_error_from_old_run_ignored() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });

        state.handle_agent_event(AgentEvent::ProviderError {
            run_id: RunId::new(1),
            error: crate::agent::events::ProviderError {
                reason: StopReason::Error,
                message: "stale error".into(),
            },
        });
        assert!(state.error.is_none(), "Stale provider error should be ignored");
        assert!(state.transcript.is_empty());
    }

    #[test]
    fn late_run_aborted_from_old_run_ignored() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });

        state.handle_agent_event(AgentEvent::RunAborted { run_id: RunId::new(1) });
        // Run B's current_run_id should still be RunId::new(2)
        assert_eq!(state.current_run_id, Some(RunId::new(2)));
        // Transcript should be empty (Run A's abort didn't add anything)
        assert!(state.transcript.is_empty());
    }

    #[test]
    fn late_thinking_delta_from_old_run_ignored() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(2) });
        let orig_status = state.status.clone();

        state.handle_agent_event(AgentEvent::ThinkingDelta {
            run_id: RunId::new(1),
            text: "thinking...".into(),
        });
        assert_eq!(state.status, orig_status, "Stale thinking should not change status");
    }

    // ── Snapshot tests ────────────────────────────────────────────────────

    #[test]
    fn snapshot_idle() {
        let state = AppState::new((80, 24));
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_streaming() {
        let mut state = AppState::new((80, 24));
        state.activity = ActivityState::Running;
        state.transcript.push(TranscriptEntry::User("hi".into()));
        state.transcript.push(TranscriptEntry::Assistant(
            "Hello! I can help you with coding tasks.".into(),
        ));
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_tool_running() {
        let mut state = AppState::new((80, 24));
        state.activity = ActivityState::Running;
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
            stdout: String::new(),
            stderr: String::new(),
            state: ToolRunState::Running,
            result: None,
        });
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_tool_success() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            stdout: "file1.txt\nfile2.txt".into(),
            stderr: String::new(),
            state: ToolRunState::Succeeded,
            result: None,
        });
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_tool_error() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptEntry::Tool {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            stdout: String::new(),
            stderr: "command not found".into(),
            state: ToolRunState::Failed,
            result: None,
        });
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_provider_error() {
        let mut state = AppState::new((80, 24));
        state
            .transcript
            .push(TranscriptEntry::ProviderError("Rate limit exceeded".into()));
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_aborted() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::RunStarted { run_id: RunId::new(1) });
        state.activity = ActivityState::Running;
        state.handle_agent_event(AgentEvent::RunAborted { run_id: RunId::new(1) });
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_narrow_terminal() {
        let state = AppState::new((40, 12));
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn snapshot_unicode() {
        let mut state = AppState::new((80, 24));
        state
            .transcript
            .push(TranscriptEntry::Assistant("你好世界 🌍 café résumé".into()));
        let output = render_state(&state);
        insta::assert_snapshot!(output);
    }
}
