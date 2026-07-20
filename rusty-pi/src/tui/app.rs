//! Ratatui TUI application state, actions, effects, update, and view.
//!
//! This is the minimal vertical slice for the TUI:
//! - Transcript area
//! - Single-line input box
//! - Status area
//! - Tool running state
//! - Error state

use crate::agent::events::AgentEvent;
use crate::ai::types::StopReason;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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
    /// Current run state.
    pub run_state: RunState,
    /// Current tool being executed (if any).
    pub current_tool: Option<ToolState>,
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
}

/// A single entry in the transcript.
#[derive(Debug, Clone)]
pub enum TranscriptEntry {
    /// A user prompt.
    User(String),
    /// An agent text response (accumulated).
    Assistant(String),
    /// A tool call started.
    ToolStarted { name: String, arguments: serde_json::Value },
    /// A tool call finished.
    ToolFinished {
        name: String,
        is_error: bool,
        output: String,
    },
    /// A provider error.
    ProviderError(String),
    /// A system message (abort, etc).
    System(String),
}

/// The state of an agent run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// Idle, waiting for input.
    Idle,
    /// Running an agent turn.
    Running,
    /// Run was aborted.
    Aborted,
}

/// State of a currently executing tool.
#[derive(Debug, Clone)]
pub struct ToolState {
    pub name: String,
    pub arguments: serde_json::Value,
}

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
            run_state: RunState::Idle,
            current_tool: None,
            status: String::from("Ready"),
            error: None,
            terminal_size,
            quit: false,
            scroll_offset: 0,
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
                                    if self.run_state == RunState::Running {
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
                        if self.run_state == RunState::Idle {
                            let prompt = self.input.trim().to_string();
                            if !prompt.is_empty() {
                                self.transcript.push(TranscriptEntry::User(prompt.clone()));
                                self.run_state = RunState::Running;
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
                if self.run_state == RunState::Idle {
                    let prompt = self.input.trim().to_string();
                    if !prompt.is_empty() {
                        self.transcript.push(TranscriptEntry::User(prompt.clone()));
                        self.run_state = RunState::Running;
                        self.status = "Running...".into();
                        self.input.clear();
                        self.cursor = 0;
                        effects.push(Effect::RunAgent(prompt));
                    }
                }
            }
            Action::Cancel => {
                if self.run_state == RunState::Running {
                    effects.push(Effect::CancelAgent);
                }
            }
            Action::AgentEvent(event) => {
                self.handle_agent_event(event);
            }
            Action::Quit => {
                self.quit = true;
                effects.push(Effect::Quit);
            }
        }

        effects
    }

    /// Handle an agent event by updating the transcript.
    fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::RunStarted => {
                // State already set to Running
            }
            AgentEvent::TextDelta { text } => {
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
            AgentEvent::ThinkingDelta { text } => {
                // Show thinking in status
                self.status = format!("Thinking: {}", text.chars().take(50).collect::<String>());
            }
            AgentEvent::ToolStarted { name, arguments, .. } => {
                self.current_tool = Some(ToolState {
                    name: name.clone(),
                    arguments: arguments.clone(),
                });
                self.transcript.push(TranscriptEntry::ToolStarted {
                    name: name.clone(),
                    arguments: serde_json::json!({}),
                });
                self.status = format!("Running {}...", name);
            }
            AgentEvent::ToolFinished { result, .. } => {
                let tool_name = self
                    .current_tool
                    .as_ref()
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| "unknown".into());
                self.current_tool = None;

                let output = result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        crate::ai::types::Content::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .next()
                    .unwrap_or("")
                    .to_string();

                self.transcript.push(TranscriptEntry::ToolFinished {
                    name: tool_name,
                    is_error: result.is_error,
                    output,
                });
                self.status = "Running...".into();
            }
            AgentEvent::ProviderError { error } => {
                self.transcript
                    .push(TranscriptEntry::ProviderError(error.message.clone()));
                self.error = Some(error.message);
            }
            AgentEvent::RunAborted => {
                self.run_state = RunState::Aborted;
                self.current_tool = None;
                self.transcript.push(TranscriptEntry::System("Run aborted".into()));
                self.status = "Aborted".into();
                // Reset to idle after a brief moment
                self.run_state = RunState::Idle;
            }
            AgentEvent::RunFinished { stop_reason } => {
                self.run_state = RunState::Idle;
                self.current_tool = None;
                match stop_reason {
                    StopReason::Stop => {
                        self.status = "Ready".into();
                    }
                    StopReason::Error => {
                        self.status = "Error".into();
                    }
                    StopReason::Length => {
                        self.status = "Truncated".into();
                    }
                    StopReason::Aborted => {
                        self.status = "Aborted".into();
                    }
                    StopReason::ToolUse => {
                        self.status = "Running...".into();
                    }
                }
            }
            AgentEvent::ToolOutput { chunk, .. } => {
                // Append to the last assistant entry or create one
                match self.transcript.last_mut() {
                    Some(TranscriptEntry::Assistant(s)) => {
                        s.push_str(&chunk);
                    }
                    _ => {
                        self.transcript.push(TranscriptEntry::Assistant(chunk));
                    }
                }
            }
        }
    }
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
            TranscriptEntry::ToolStarted { name, .. } => {
                lines.push(Line::from(vec![
                    Span::styled(format!("⚙ {} ", name), Style::default().fg(Color::Yellow)),
                    Span::styled("running...", Style::default().fg(Color::DarkGray)),
                ]));
            }
            TranscriptEntry::ToolFinished { name, is_error, output } => {
                let style = if *is_error {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                };
                let icon = if *is_error { "❌" } else { "✅" };
                lines.push(Line::from(vec![Span::styled(format!("{} {} ", icon, name), style)]));
                // Show first few lines of output
                for line in output.lines().take(3) {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
                if output.lines().count() > 3 {
                    lines.push(Line::from(vec![Span::styled(
                        "  ...",
                        Style::default().fg(Color::DarkGray),
                    )]));
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
    let display_text = if state.run_state == RunState::Running {
        format!("{} ⏳", state.input)
    } else {
        state.input.clone()
    };

    let paragraph = Paragraph::new(display_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Input")
                .border_style(Style::default().fg(if state.run_state == RunState::Running {
                    Color::Yellow
                } else {
                    Color::Cyan
                })),
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
    } else if state.run_state == RunState::Running {
        Style::default().fg(Color::Yellow)
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
    use crossterm::event::{KeyCode, KeyModifiers};

    fn key_event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn app_state_initial() {
        let state = AppState::new((80, 24));
        assert_eq!(state.run_state, RunState::Idle);
        assert!(state.input.is_empty());
        assert!(state.transcript.is_empty());
        assert!(!state.quit);
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
        assert_eq!(state.run_state, RunState::Idle);
    }

    #[test]
    fn app_state_submit_creates_run() {
        let mut state = AppState::new((80, 24));
        state.update(Action::KeyInput(key_event(KeyCode::Char('h'), KeyModifiers::NONE)));
        state.update(Action::KeyInput(key_event(KeyCode::Char('i'), KeyModifiers::NONE)));
        let effects = state.update(Action::KeyInput(key_event(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(state.run_state, RunState::Running);
        assert_eq!(state.transcript.len(), 1);
        assert!(matches!(&state.transcript[0], TranscriptEntry::User(s) if s == "hi"));
        assert!(!effects.is_empty());
    }

    #[test]
    fn app_state_cancel_while_running() {
        let mut state = AppState::new((80, 24));
        state.run_state = RunState::Running;
        let effects = state.update(Action::Cancel);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::CancelAgent));
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
        state.handle_agent_event(AgentEvent::TextDelta { text: "hello".into() });
        state.handle_agent_event(AgentEvent::TextDelta { text: " world".into() });
        assert_eq!(state.transcript.len(), 1);
        match &state.transcript[0] {
            TranscriptEntry::Assistant(s) => assert_eq!(s, "hello world"),
            _ => panic!("Expected Assistant entry"),
        }
    }

    #[test]
    fn app_state_tool_started_updates_status() {
        let mut state = AppState::new((80, 24));
        state.handle_agent_event(AgentEvent::ToolStarted {
            id: "tc_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        });
        assert!(state.current_tool.is_some());
        assert!(state.status.contains("bash"));
    }

    #[test]
    fn app_state_tool_finished_clears_tool_state() {
        let mut state = AppState::new((80, 24));
        state.current_tool = Some(ToolState {
            name: "bash".into(),
            arguments: serde_json::json!({}),
        });
        state.handle_agent_event(AgentEvent::ToolFinished {
            id: "tc_1".into(),
            result: crate::agent::types::AgentToolResult {
                content: vec![crate::ai::types::Content::Text { text: "output".into() }],
                ..Default::default()
            },
        });
        assert!(state.current_tool.is_none());
    }

    #[test]
    fn app_state_run_finished_sets_idle() {
        let mut state = AppState::new((80, 24));
        state.run_state = RunState::Running;
        state.handle_agent_event(AgentEvent::RunFinished {
            stop_reason: StopReason::Stop,
        });
        assert_eq!(state.run_state, RunState::Idle);
    }

    #[test]
    fn app_state_quit() {
        let mut state = AppState::new((80, 24));
        let effects = state.update(Action::Quit);
        assert!(state.quit);
        assert!(matches!(&effects[0], Effect::Quit));
    }
}
