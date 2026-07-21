//! Ratatui TUI state, reducer, and rendering.
//!
//! The agent/session layers only emit domain events.  This module turns those
//! events into a small, stable view model and routes keyboard input through a
//! reducer.  Rendering is deliberately read-only: all lifecycle, selection,
//! history, and scroll changes happen in [`AppState::update`].

use crate::agent::events::{AgentEvent, RunId, ToolOutputStream};
use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;
use crate::coding_agent::command::{CommandControl, CommandHelpItem, CommandOutcome, CommandResult};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthChar;

/// Maximum retained output for either tool stream.
///
/// Output is bounded before it enters the transcript, rather than only while
/// drawing, so a noisy command cannot make AppState grow without limit.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;
const OUTPUT_TRUNCATION_MARKER: &str = "… output truncated …";
const MAX_TRANSCRIPT_BLOCKS: usize = 2_000;

// ── View model ──────────────────────────────────────────────────────────────

/// The state of a tool invocation as shown in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRunState {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Aborted,
}

impl ToolRunState {
    fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "success",
            Self::Failed => "failed",
            Self::TimedOut => "timed out",
            Self::Aborted => "aborted",
        }
    }
}

/// Structured content rendered by the TUI.
///
/// This is a frontend view model.  It is intentionally not part of the agent
/// or session model: tool output, provider errors, and command results never
/// become assistant text.
#[derive(Debug, Clone, PartialEq)]
pub enum TranscriptBlock {
    User {
        text: String,
    },
    Assistant {
        text: String,
        streaming: bool,
    },
    Thinking {
        text: String,
        expanded: bool,
    },
    Tool {
        tool_call_id: String,
        name: String,
        arguments: serde_json::Value,
        /// Formatted when ToolStarted is reduced, not in view().
        arguments_display: String,
        stdout: String,
        stderr: String,
        state: ToolRunState,
        expanded: bool,
        exit_code: Option<i32>,
        duration: Option<Duration>,
    },
    Error {
        message: String,
    },
    System {
        message: String,
    },
}

/// Compatibility name for callers that used the initial TUI prototype.
pub type TranscriptEntry = TranscriptBlock;

impl TranscriptBlock {
    fn tool(tool_call_id: String, name: String, arguments: serde_json::Value) -> Self {
        let arguments_display = sanitize_message(
            &serde_json::to_string_pretty(&arguments).unwrap_or_else(|_| "<invalid arguments>".into()),
        );
        Self::Tool {
            tool_call_id,
            name,
            arguments,
            arguments_display,
            stdout: String::new(),
            stderr: String::new(),
            state: ToolRunState::Running,
            expanded: false,
            exit_code: None,
            duration: None,
        }
    }

    fn is_expandable(&self) -> bool {
        matches!(self, Self::Tool { .. } | Self::Thinking { .. })
    }
}

/// Explicit conversion seam for a future session/domain transcript.
///
/// The current session transcript is reconstructed by AgentEvent updates, so
/// the input and output are already frontend blocks.  Keeping this function
/// public makes that boundary explicit and gives a future persisted transcript
/// one place to map into Ratatui's model.
pub fn transcript_to_blocks(entries: &[TranscriptEntry]) -> Vec<TranscriptBlock> {
    entries.to_vec()
}

// ── Input model ─────────────────────────────────────────────────────────────

/// UTF-8 safe multiline editor state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub text: String,
    pub cursor_byte: usize,
    pub preferred_column: Option<usize>,
    /// Vertical scroll in wrapped display lines.
    pub viewport_line: usize,
}

impl InputState {
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor_byte = self.text.len();
        self.preferred_column = None;
        self.viewport_line = 0;
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor_byte = 0;
        self.preferred_column = None;
        self.viewport_line = 0;
    }

    fn insert_char(&mut self, c: char) {
        debug_assert!(self.text.is_char_boundary(self.cursor_byte));
        self.text.insert(self.cursor_byte, c);
        self.cursor_byte += c.len_utf8();
        self.preferred_column = None;
    }

    fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        debug_assert!(self.text.is_char_boundary(self.cursor_byte));
        self.text.insert_str(self.cursor_byte, text);
        self.cursor_byte += text.len();
        self.preferred_column = None;
    }

    fn previous_boundary(&self) -> Option<usize> {
        self.text[..self.cursor_byte].char_indices().next_back().map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor_byte..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor_byte + i)
    }

    fn move_left(&mut self) {
        if let Some(previous) = self.previous_boundary() {
            self.cursor_byte = previous;
        }
        self.preferred_column = None;
    }

    fn move_right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.cursor_byte = next;
        }
        self.preferred_column = None;
    }

    fn line_start(&self, byte: usize) -> usize {
        self.text[..byte].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, byte: usize) -> usize {
        self.text[byte..].find('\n').map_or(self.text.len(), |i| byte + i)
    }

    fn current_column(&self) -> usize {
        self.text[self.line_start(self.cursor_byte)..self.cursor_byte]
            .chars()
            .map(char_width)
            .sum()
    }

    fn line_bounds(&self, line: usize) -> Option<(usize, usize)> {
        let mut current = 0;
        let mut start = 0;
        for (index, character) in self.text.char_indices() {
            if current == line {
                if character == '\n' {
                    return Some((start, index));
                }
            } else if character == '\n' {
                current += 1;
                start = index + 1;
            }
        }
        (current == line).then_some((start, self.text.len()))
    }

    fn current_line(&self) -> usize {
        self.text[..self.cursor_byte]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
    }

    fn move_home(&mut self) {
        self.cursor_byte = self.line_start(self.cursor_byte);
        self.preferred_column = None;
    }

    fn move_end(&mut self) {
        self.cursor_byte = self.line_end(self.cursor_byte);
        self.preferred_column = None;
    }

    fn move_vertical(&mut self, direction: i32) {
        let current_line = self.current_line();
        let target_line = if direction < 0 {
            current_line.saturating_sub(1)
        } else {
            current_line.saturating_add(1)
        };
        let Some((start, end)) = self.line_bounds(target_line) else {
            return;
        };
        let desired = self.preferred_column.unwrap_or_else(|| self.current_column());
        let mut column = 0;
        let mut cursor = start;
        for (offset, character) in self.text[start..end].char_indices() {
            let width = char_width(character);
            if column + width > desired {
                break;
            }
            column += width;
            cursor = start + offset + character.len_utf8();
        }
        self.cursor_byte = cursor;
        self.preferred_column = Some(desired);
    }

    fn delete_to_line_start(&mut self) {
        let start = self.line_start(self.cursor_byte);
        self.text.drain(start..self.cursor_byte);
        self.cursor_byte = start;
        self.preferred_column = None;
    }

    fn delete_to_line_end(&mut self) {
        let end = self.line_end(self.cursor_byte);
        self.text.drain(self.cursor_byte..end);
        self.preferred_column = None;
    }

    fn delete_previous_word(&mut self) {
        let mut start = self.cursor_byte;
        let mut saw_non_whitespace = false;
        while let Some(previous) = self.text[..start].char_indices().next_back() {
            let character = previous.1;
            if character.is_whitespace() {
                if saw_non_whitespace {
                    break;
                }
            } else {
                saw_non_whitespace = true;
            }
            start = previous.0;
        }
        self.text.drain(start..self.cursor_byte);
        self.cursor_byte = start;
        self.preferred_column = None;
    }

    fn backspace(&mut self) {
        if let Some(previous) = self.previous_boundary() {
            self.text.drain(previous..self.cursor_byte);
            self.cursor_byte = previous;
            self.preferred_column = None;
        }
    }

    fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.drain(self.cursor_byte..next);
            self.preferred_column = None;
        }
    }

    fn cursor_visual_position(&self, width: usize) -> (usize, usize) {
        visual_position(&self.text, self.cursor_byte, width)
    }
}

fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0).max(1)
}

/// Prompt history for the current process only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputHistory {
    pub entries: Vec<String>,
    pub cursor: Option<usize>,
    pub draft: String,
}

impl InputHistory {
    fn record(&mut self, prompt: &str) {
        if prompt.trim().is_empty() {
            return;
        }
        if self.entries.last().is_none_or(|last| last != prompt) {
            self.entries.push(prompt.to_owned());
        }
        self.cursor = None;
        self.draft.clear();
    }

    fn previous(&mut self, input: &mut InputState) {
        if self.entries.is_empty() {
            return;
        }
        if self.cursor.is_none() {
            self.draft = input.text.clone();
            self.cursor = Some(self.entries.len() - 1);
        } else if let Some(cursor) = self.cursor {
            self.cursor = Some(cursor.saturating_sub(1));
        }
        if let Some(cursor) = self.cursor {
            input.set_text(self.entries[cursor].clone());
        }
    }

    fn next(&mut self, input: &mut InputState) {
        let Some(cursor) = self.cursor else {
            return;
        };
        if cursor + 1 < self.entries.len() {
            self.cursor = Some(cursor + 1);
            input.set_text(self.entries[cursor + 1].clone());
        } else {
            self.cursor = None;
            input.set_text(self.draft.clone());
            self.draft.clear();
        }
    }
}

// ── Focus and scroll state ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Transcript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptSelection {
    pub selected_block: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollState {
    pub offset_from_bottom: usize,
    pub follow_output: bool,
    pub unread_lines: usize,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            offset_from_bottom: 0,
            follow_output: true,
            unread_lines: 0,
        }
    }
}

// ── App state and reducer ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityState {
    Idle,
    AgentRunning,
    AgentCancelling,
    CommandRunning,
    CommandCancelling,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Aborted,
    ProviderError(String),
    ToolError(String),
}

/// Complete TUI state.  This is the only state changed by the reducer.
#[derive(Debug, Clone)]
pub struct AppState {
    pub transcript: Vec<TranscriptBlock>,
    pub input: InputState,
    pub history: InputHistory,
    pub focus: Focus,
    pub selection: TranscriptSelection,
    pub scroll: ScrollState,
    pub activity: ActivityState,
    pub last_outcome: Option<RunOutcome>,
    pub current_run_id: Option<RunId>,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub error: Option<String>,
    pub terminal_size: (u16, u16),
    pub quit: bool,
    tool_index: HashMap<String, usize>,
    tool_started_at: HashMap<String, Instant>,
    run_tool_error: Option<String>,
}

#[derive(Debug)]
pub enum Action {
    KeyInput(KeyEvent),
    Paste(String),
    Resize(u16, u16),
    Submit,
    Cancel,
    AgentPromptStarted { original: String, expanded: String },
    CommandStarted { name: String },
    CommandCompleted(CommandOutcome),
    CommandCancelled,
    CommandFailed(String),
    InputRouteError(String),
    AgentEvent(AgentEvent),
    Quit,
}

#[derive(Debug)]
pub enum Effect {
    SubmitInput(String),
    RunAgent(String),
    CancelAgent,
    CancelCommand,
    Quit,
}

impl AppState {
    pub fn new(terminal_size: (u16, u16)) -> Self {
        Self {
            transcript: Vec::new(),
            input: InputState::default(),
            history: InputHistory::default(),
            focus: Focus::Input,
            selection: TranscriptSelection { selected_block: None },
            scroll: ScrollState::default(),
            activity: ActivityState::Idle,
            last_outcome: None,
            current_run_id: None,
            provider: "mock".into(),
            model: "mock".into(),
            status: "Ready".into(),
            error: None,
            terminal_size,
            quit: false,
            tool_index: HashMap::new(),
            tool_started_at: HashMap::new(),
            run_tool_error: None,
        }
    }

    pub fn set_provider_model(&mut self, provider: impl Into<String>, model: impl Into<String>) {
        self.provider = provider.into();
        self.model = model.into();
    }

    pub fn update(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::KeyInput(key) => self.handle_key(key),
            Action::Paste(text) => {
                if self.focus == Focus::Input {
                    self.input.insert_text(&text);
                    self.sync_input_viewport();
                }
                Vec::new()
            }
            Action::Resize(width, height) => {
                self.terminal_size = (width, height);
                self.clamp_scroll();
                self.sync_input_viewport();
                Vec::new()
            }
            Action::Submit => self.submit(),
            Action::Cancel => self.cancel(),
            Action::AgentPromptStarted { original, expanded } => self.agent_prompt_started(original, expanded),
            Action::CommandStarted { name } => {
                self.activity = ActivityState::CommandRunning;
                self.error = None;
                self.last_outcome = None;
                self.status = format!("Command /{name}");
                Vec::new()
            }
            Action::CommandCompleted(outcome) => self.apply_command_outcome(outcome),
            Action::CommandCancelled => self.command_cancelled(),
            Action::CommandFailed(message) | Action::InputRouteError(message) => self.command_failed(message),
            Action::AgentEvent(event) => {
                self.handle_agent_event(event);
                Vec::new()
            }
            Action::Quit => {
                self.quit = true;
                vec![Effect::Quit]
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.error = None;

        // These routes are global so input focus cannot accidentally consume
        // transcript navigation keys.
        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Input => {
                        self.selection.selected_block = self.last_selectable();
                        Focus::Transcript
                    }
                    Focus::Transcript => Focus::Input,
                };
                return Vec::new();
            }
            KeyCode::PageUp => {
                self.scroll_by(10, true);
                return Vec::new();
            }
            KeyCode::PageDown => {
                self.scroll_by(10, false);
                return Vec::new();
            }
            KeyCode::Esc => {
                self.selection.selected_block = None;
                self.focus = Focus::Input;
                return Vec::new();
            }
            _ => {}
        }

        if self.focus == Focus::Transcript {
            return self.handle_transcript_key(key);
        }
        self.handle_input_key(key)
    }

    fn handle_transcript_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('i') => {
                self.focus = Focus::Input;
            }
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::Home => {
                self.scroll_to_start();
                self.selection.selected_block = self.first_selectable();
            }
            KeyCode::End => {
                self.scroll_to_end();
                self.selection.selected_block = self.last_selectable();
            }
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_selected(),
            _ => {}
        }
        Vec::new()
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.handle_ctrl_c();
        }

        if key.code == KeyCode::Enter
            && (key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::ALT))
        {
            self.input.insert_char('\n');
            self.sync_input_viewport();
            return Vec::new();
        }

        if let KeyCode::Char(character) = key.code {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match character {
                    'a' => self.input.move_home(),
                    'e' => self.input.move_end(),
                    'j' => self.input.insert_char('\n'),
                    'u' => self.input.delete_to_line_start(),
                    'k' => self.input.delete_to_line_end(),
                    'w' => self.input.delete_previous_word(),
                    _ => return Vec::new(),
                }
                self.sync_input_viewport();
                return Vec::new();
            }
            self.input.insert_char(character);
            self.sync_input_viewport();
            return Vec::new();
        }

        match key.code {
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.backspace();
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Delete => {
                self.input.delete();
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Left => {
                self.input.move_left();
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Right => {
                self.input.move_right();
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Up => {
                if self.input.text.is_empty() || can_browse_history(&self.input) {
                    self.history.previous(&mut self.input);
                } else {
                    self.input.move_vertical(-1);
                }
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Down => {
                if self.history.cursor.is_some() {
                    self.history.next(&mut self.input);
                } else {
                    self.input.move_vertical(1);
                }
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::Home => {
                self.input.move_home();
                self.sync_input_viewport();
                Vec::new()
            }
            KeyCode::End => {
                // End edits the current line and also has the useful global
                // meaning of returning the transcript to the latest output.
                self.input.move_end();
                self.scroll_to_end();
                self.sync_input_viewport();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn handle_ctrl_c(&mut self) -> Vec<Effect> {
        match self.activity {
            ActivityState::AgentRunning => self.cancel(),
            ActivityState::AgentCancelling | ActivityState::CommandCancelling => Vec::new(),
            ActivityState::CommandRunning => {
                self.activity = ActivityState::CommandCancelling;
                self.status = "Command cancelling · waiting".into();
                vec![Effect::CancelCommand]
            }
            ActivityState::Idle if !self.input.text.is_empty() => {
                self.input.clear();
                self.history.cursor = None;
                self.history.draft.clear();
                self.status = "Input cleared".into();
                Vec::new()
            }
            ActivityState::Idle => Vec::new(),
        }
    }

    fn submit(&mut self) -> Vec<Effect> {
        if self.activity != ActivityState::Idle {
            self.status = "Busy · Enter unavailable · Ctrl+C to cancel".into();
            return Vec::new();
        }
        let prompt = self.input.text.clone();
        if prompt.trim().is_empty() {
            return Vec::new();
        }
        self.history.record(&prompt);
        self.input.clear();
        self.focus = Focus::Input;
        self.last_outcome = None;
        self.run_tool_error = None;
        self.status = "Dispatching".into();
        vec![Effect::SubmitInput(prompt)]
    }

    fn cancel(&mut self) -> Vec<Effect> {
        if self.activity == ActivityState::AgentRunning {
            self.activity = ActivityState::AgentCancelling;
            self.status = "Cancelling · waiting for agent".into();
            vec![Effect::CancelAgent]
        } else {
            Vec::new()
        }
    }

    fn agent_prompt_started(&mut self, original: String, expanded: String) -> Vec<Effect> {
        self.transcript.push(TranscriptBlock::User { text: original });
        self.enforce_block_limit();
        self.activity = ActivityState::AgentRunning;
        self.last_outcome = None;
        self.run_tool_error = None;
        self.status = "Agent running".into();
        self.note_new_lines(2);
        vec![Effect::RunAgent(expanded)]
    }

    fn apply_command_outcome(&mut self, outcome: CommandOutcome) -> Vec<Effect> {
        if outcome.control == CommandControl::Quit {
            self.activity = ActivityState::Idle;
            self.quit = true;
            return vec![Effect::Quit];
        }
        self.activity = ActivityState::Idle;
        if let Some(result) = outcome.result {
            self.apply_command_result(result)
        } else {
            self.error = None;
            self.status = "Ready".into();
            Vec::new()
        }
    }

    fn command_cancelled(&mut self) -> Vec<Effect> {
        if matches!(
            self.activity,
            ActivityState::CommandRunning | ActivityState::CommandCancelling
        ) {
            self.activity = ActivityState::Idle;
            self.error = None;
            self.status = "Command cancelled".into();
            self.transcript.push(TranscriptBlock::System {
                message: "Command cancelled".into(),
            });
            self.note_new_lines(2);
        }
        Vec::new()
    }

    fn command_failed(&mut self, message: String) -> Vec<Effect> {
        self.activity = ActivityState::Idle;
        let message = sanitize_message(&message);
        self.error = Some(message.clone());
        self.status = "Command error".into();
        self.transcript.push(TranscriptBlock::Error { message });
        self.note_new_lines(2);
        self.enforce_block_limit();
        Vec::new()
    }

    pub fn apply_command_result(&mut self, result: CommandResult) -> Vec<Effect> {
        self.activity = ActivityState::Idle;
        self.error = None;
        let block = match result {
            CommandResult::Message(message) => TranscriptBlock::System { message },
            CommandResult::Error(message) => {
                let message = sanitize_message(&message);
                self.error = Some(message.clone());
                self.status = "Command error".into();
                TranscriptBlock::Error { message }
            }
            CommandResult::Help(items) => TranscriptBlock::System {
                message: format_help(&items),
            },
            CommandResult::ModelChanged { model } => TranscriptBlock::System {
                message: format!("Switched to {model}"),
            },
            CommandResult::Sessions { sessions, skipped } => TranscriptBlock::System {
                message: if sessions.is_empty() {
                    if skipped == 0 {
                        "No sessions found.".into()
                    } else {
                        format!("No sessions found.\nSkipped {skipped} unreadable session files.")
                    }
                } else {
                    let mut text = String::from("Available sessions:");
                    for session in sessions {
                        text.push_str(&format!(
                            "\n  {} | model: {} | msgs: {} | created: {}",
                            session.id, session.model, session.msg_count, session.created
                        ));
                    }
                    if skipped > 0 {
                        text.push_str(&format!("\nSkipped {skipped} unreadable session files."));
                    }
                    text
                },
            },
        };
        let line_count = block_line_count(&block);
        self.transcript.push(block);
        self.enforce_block_limit();
        self.note_new_lines(line_count);
        if self.error.is_none() {
            self.status = "Ready".into();
        }
        Vec::new()
    }

    /// Reduce one domain event into the frontend transcript.
    /// Events from an older run are ignored by RunId, never appended to a new
    /// run's blocks.
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::RunStarted { run_id } => {
                self.current_run_id = Some(run_id);
                self.activity = ActivityState::AgentRunning;
                self.tool_index.clear();
                self.tool_started_at.clear();
                self.run_tool_error = None;
            }
            AgentEvent::TextDelta { run_id, text } if self.accepts(run_id) => {
                if text.is_empty() {
                    return;
                }
                match self.transcript.last_mut() {
                    Some(TranscriptBlock::Assistant {
                        text: current,
                        streaming,
                    }) => {
                        current.push_str(&text);
                        *streaming = true;
                    }
                    _ => self.transcript.push(TranscriptBlock::Assistant {
                        text: text.clone(),
                        streaming: true,
                    }),
                }
                self.note_new_lines(text_line_count(&text));
                self.enforce_block_limit();
            }
            AgentEvent::ThinkingDelta { run_id, text } if self.accepts(run_id) => {
                if text.is_empty() {
                    return;
                }
                match self.transcript.last_mut() {
                    Some(TranscriptBlock::Thinking { text: current, .. }) => current.push_str(&text),
                    _ => self.transcript.push(TranscriptBlock::Thinking {
                        text: text.clone(),
                        expanded: false,
                    }),
                }
                self.status = format!("Thinking · {} lines", text_line_count(&text));
                self.note_new_lines(text_line_count(&text));
                self.enforce_block_limit();
            }
            AgentEvent::ToolStarted {
                run_id,
                tool_call_id,
                name,
                arguments,
            } if self.accepts(run_id) => {
                let index = self.transcript.len();
                self.tool_index.insert(tool_call_id.clone(), index);
                self.tool_started_at.insert(tool_call_id.clone(), Instant::now());
                self.transcript
                    .push(TranscriptBlock::tool(tool_call_id, name.clone(), arguments));
                self.status = format!("Running {name}");
                self.note_new_lines(1);
                self.enforce_block_limit();
            }
            AgentEvent::ToolOutput {
                run_id,
                tool_call_id,
                stream,
                chunk,
            } if self.accepts(run_id) => {
                self.reduce_tool_output(tool_call_id, stream, &chunk);
                self.note_new_lines(text_line_count(&chunk));
            }
            AgentEvent::ToolFinished {
                run_id,
                tool_call_id,
                name,
                result,
            } if self.accepts(run_id) => {
                let tool_state = tool_state_for(&result);
                let duration = self.tool_started_at.remove(&tool_call_id).map(|start| start.elapsed());
                let final_text = result
                    .content
                    .iter()
                    .filter_map(|content| match content {
                        crate::ai::types::Content::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some(&index) = self.tool_index.get(&tool_call_id) {
                    if let Some(TranscriptBlock::Tool {
                        name: entry_name,
                        state,
                        exit_code,
                        duration: entry_duration,
                        stdout,
                        stderr,
                        ..
                    }) = self.transcript.get_mut(index)
                    {
                        if entry_name == "unknown" {
                            *entry_name = name.clone();
                        }
                        *state = tool_state;
                        *exit_code = result.exit_code;
                        *entry_duration = duration;
                        // Most tools stream their result.  For tools that do
                        // not, retain the final text once rather than adding
                        // a duplicate after ToolOutput chunks.
                        if stdout.is_empty() && stderr.is_empty() && !final_text.is_empty() {
                            append_limited(stdout, &final_text);
                        }
                    }
                } else {
                    let index = self.transcript.len();
                    self.tool_index.insert(tool_call_id.clone(), index);
                    let mut block = TranscriptBlock::tool(tool_call_id, name.clone(), serde_json::Value::Null);
                    if let TranscriptBlock::Tool {
                        state,
                        exit_code,
                        duration: entry_duration,
                        stdout,
                        stderr,
                        ..
                    } = &mut block
                    {
                        *state = tool_state;
                        *exit_code = result.exit_code;
                        *entry_duration = duration;
                        if stdout.is_empty() && stderr.is_empty() && !final_text.is_empty() {
                            append_limited(stdout, &final_text);
                        }
                    }
                    self.transcript.push(block);
                    self.note_new_lines(1);
                }
                if tool_state == ToolRunState::Failed {
                    self.run_tool_error = Some(format!("{} failed", name));
                    self.status = format!("Tool error · {name}");
                } else {
                    self.status = format!("{} · {}", name, tool_state.label());
                }
            }
            AgentEvent::ProviderError { run_id, error } if self.accepts(run_id) => {
                let message = sanitize_message(&error.message);
                self.error = Some(message.clone());
                self.transcript.push(TranscriptBlock::Error { message });
                self.note_new_lines(2);
                self.enforce_block_limit();
            }
            AgentEvent::RunAborted { run_id } if self.accepts(run_id) => {
                self.finish_assistant_stream();
                self.activity = ActivityState::Idle;
                self.last_outcome = Some(RunOutcome::Aborted);
                self.transcript.push(TranscriptBlock::System {
                    message: "Run aborted".into(),
                });
                self.status = "Aborted".into();
                self.note_new_lines(2);
                self.enforce_block_limit();
            }
            AgentEvent::RunFailed { run_id, error } if self.accepts(run_id) => {
                self.finish_assistant_stream();
                self.activity = ActivityState::Idle;
                self.last_outcome = Some(RunOutcome::ToolError(format!("{}: {}", error.phase, error.message)));
                self.transcript.push(TranscriptBlock::Error {
                    message: format!("Run failed [{}]: {}", error.phase, error.message),
                });
                self.status = format!("Failed · {}", error.phase);
                self.note_new_lines(2);
                self.enforce_block_limit();
            }
            AgentEvent::RunFinished { run_id, stop_reason } if self.accepts(run_id) => {
                self.finish_assistant_stream();
                match stop_reason {
                    StopReason::ToolUse => {
                        self.activity = ActivityState::AgentRunning;
                        self.status = "Agent running".into();
                    }
                    StopReason::Stop => {
                        self.activity = ActivityState::Idle;
                        self.last_outcome = self
                            .run_tool_error
                            .take()
                            .map(RunOutcome::ToolError)
                            .or(Some(RunOutcome::Completed));
                        self.status = "Ready".into();
                    }
                    StopReason::Length => {
                        self.activity = ActivityState::Idle;
                        self.last_outcome = Some(RunOutcome::Completed);
                        self.status = "Completed · response truncated".into();
                    }
                    StopReason::Error => {
                        self.activity = ActivityState::Idle;
                        self.last_outcome = Some(RunOutcome::ProviderError(
                            self.error.clone().unwrap_or_else(|| "Provider error".into()),
                        ));
                        self.status = "Provider error".into();
                    }
                    StopReason::Aborted => {
                        self.activity = ActivityState::Idle;
                        self.last_outcome = Some(RunOutcome::Aborted);
                        self.status = "Aborted".into();
                    }
                }
            }
            _ => {}
        }
    }

    fn accepts(&self, run_id: RunId) -> bool {
        self.current_run_id == Some(run_id)
    }

    fn finish_assistant_stream(&mut self) {
        if let Some(TranscriptBlock::Assistant { streaming, .. }) = self.transcript.last_mut() {
            *streaming = false;
        }
    }

    fn reduce_tool_output(&mut self, tool_call_id: String, stream: ToolOutputStream, chunk: &str) {
        let index = if let Some(index) = self.tool_index.get(&tool_call_id).copied() {
            index
        } else {
            let index = self.transcript.len();
            self.tool_index.insert(tool_call_id.clone(), index);
            self.transcript.push(TranscriptBlock::tool(
                tool_call_id,
                "unknown".into(),
                serde_json::Value::Null,
            ));
            index
        };
        if let Some(TranscriptBlock::Tool { stdout, stderr, .. }) = self.transcript.get_mut(index) {
            match stream {
                ToolOutputStream::Stdout => append_limited(stdout, chunk),
                ToolOutputStream::Stderr => append_limited(stderr, chunk),
            }
        }
        self.enforce_block_limit();
    }

    fn enforce_block_limit(&mut self) {
        if self.transcript.len() <= MAX_TRANSCRIPT_BLOCKS {
            return;
        }
        let remove = self.transcript.len() - MAX_TRANSCRIPT_BLOCKS;
        self.transcript.drain(0..remove);
        for index in self.tool_index.values_mut() {
            *index = index.saturating_sub(remove);
        }
        self.tool_index.retain(|_, index| *index < self.transcript.len());
        self.selection.selected_block = self
            .selection
            .selected_block
            .and_then(|index| (index >= remove).then_some(index - remove));
    }

    fn note_new_lines(&mut self, count: usize) {
        if !self.scroll.follow_output {
            self.scroll.unread_lines = self.scroll.unread_lines.saturating_add(count.max(1));
        } else {
            self.scroll.offset_from_bottom = 0;
            self.scroll.unread_lines = 0;
        }
    }

    pub fn scroll_by(&mut self, amount: usize, towards_start: bool) {
        if towards_start {
            self.scroll.offset_from_bottom = self.scroll.offset_from_bottom.saturating_add(amount);
            self.scroll.follow_output = false;
        } else {
            self.scroll.offset_from_bottom = self.scroll.offset_from_bottom.saturating_sub(amount);
            if self.scroll.offset_from_bottom == 0 {
                self.scroll.follow_output = true;
                self.scroll.unread_lines = 0;
            }
        }
        self.clamp_scroll();
    }

    pub fn scroll_to_start(&mut self) {
        self.scroll.offset_from_bottom = self
            .total_transcript_lines()
            .saturating_sub(self.transcript_viewport_lines());
        self.scroll.follow_output = false;
    }

    pub fn scroll_to_end(&mut self) {
        self.scroll.offset_from_bottom = 0;
        self.scroll.follow_output = true;
        self.scroll.unread_lines = 0;
    }

    fn clamp_scroll(&mut self) {
        let max = self
            .total_transcript_lines()
            .saturating_sub(self.transcript_viewport_lines());
        self.scroll.offset_from_bottom = self.scroll.offset_from_bottom.min(max);
        if self.scroll.offset_from_bottom == 0 && self.scroll.follow_output {
            self.scroll.unread_lines = 0;
        }
    }

    fn transcript_viewport_lines(&self) -> usize {
        let input = input_height(self.terminal_size.0, self.terminal_size.1, &self.input);
        self.terminal_size.1.saturating_sub(input).saturating_sub(3).max(1) as usize
    }

    fn total_transcript_lines(&self) -> usize {
        render_lines(self)
            .iter()
            .map(|line| wrapped_line_count(&line.text, self.transcript_width()))
            .sum()
    }

    fn transcript_width(&self) -> usize {
        self.terminal_size.0.saturating_sub(2).max(1) as usize
    }

    fn sync_input_viewport(&mut self) {
        let width = self.terminal_size.0.saturating_sub(2).max(1) as usize;
        let (line, _) = self.input.cursor_visual_position(width);
        let visible = input_height(self.terminal_size.0, self.terminal_size.1, &self.input).saturating_sub(2) as usize;
        let visible = visible.max(1);
        if line < self.input.viewport_line {
            self.input.viewport_line = line;
        } else if line >= self.input.viewport_line + visible {
            self.input.viewport_line = line + 1 - visible;
        }
    }

    fn first_selectable(&self) -> Option<usize> {
        self.transcript.iter().position(TranscriptBlock::is_expandable)
    }

    fn last_selectable(&self) -> Option<usize> {
        self.transcript.iter().rposition(TranscriptBlock::is_expandable)
    }

    fn move_selection(&mut self, direction: i32) {
        let candidates: Vec<usize> = self
            .transcript
            .iter()
            .enumerate()
            .filter_map(|(index, block)| block.is_expandable().then_some(index))
            .collect();
        if candidates.is_empty() {
            return;
        }
        let current = self
            .selection
            .selected_block
            .and_then(|selected| candidates.iter().position(|index| *index == selected));
        let next = match (current, direction) {
            (None, -1) => candidates.len() - 1,
            (None, _) => 0,
            (Some(position), -1) => position.saturating_sub(1),
            (Some(position), _) => (position + 1).min(candidates.len() - 1),
        };
        self.selection.selected_block = Some(candidates[next]);
    }

    fn toggle_selected(&mut self) {
        let Some(index) = self.selection.selected_block else {
            return;
        };
        match self.transcript.get_mut(index) {
            Some(TranscriptBlock::Tool { expanded, .. }) | Some(TranscriptBlock::Thinking { expanded, .. }) => {
                *expanded = !*expanded;
            }
            _ => {}
        }
    }
}

fn tool_state_for(result: &AgentToolResult) -> ToolRunState {
    if result.timed_out {
        ToolRunState::TimedOut
    } else if result.aborted {
        ToolRunState::Aborted
    } else if result.is_error {
        ToolRunState::Failed
    } else {
        ToolRunState::Succeeded
    }
}

fn text_line_count(text: &str) -> usize {
    text.split('\n').count().max(1)
}

fn can_browse_history(input: &InputState) -> bool {
    !input.text.contains('\n') && input.current_line() == 0
}

fn block_line_count(block: &TranscriptBlock) -> usize {
    match block {
        TranscriptBlock::User { text }
        | TranscriptBlock::Assistant { text, .. }
        | TranscriptBlock::Error { message: text }
        | TranscriptBlock::System { message: text } => text_line_count(text) + 1,
        TranscriptBlock::Thinking { text, expanded } => {
            if *expanded {
                text_line_count(text) + 1
            } else {
                1
            }
        }
        TranscriptBlock::Tool {
            expanded,
            stdout,
            stderr,
            ..
        } => {
            if *expanded {
                5 + text_line_count(stdout) + text_line_count(stderr)
            } else {
                1
            }
        }
    }
}

fn append_limited(output: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    let without_marker = output.replace(OUTPUT_TRUNCATION_MARKER, "");
    let combined = format!("{without_marker}{chunk}");
    if combined.len() <= MAX_TOOL_OUTPUT_BYTES {
        *output = combined;
        return;
    }
    let available = MAX_TOOL_OUTPUT_BYTES.saturating_sub(OUTPUT_TRUNCATION_MARKER.len());
    let head_budget = available / 2;
    let tail_budget = available.saturating_sub(head_budget);
    let head = safe_prefix(&combined, head_budget);
    let tail = safe_suffix(&combined, tail_budget);
    *output = format!("{head}{OUTPUT_TRUNCATION_MARKER}{tail}");
}

fn safe_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn safe_suffix(text: &str, max_bytes: usize) -> &str {
    let mut start = text.len().saturating_sub(max_bytes);
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn sanitize_message(message: &str) -> String {
    let sensitive = [
        "authorization",
        "api_key",
        "apikey",
        "access_token",
        "bearer ",
        "token=",
    ];
    message
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if sensitive.iter().any(|needle| lower.contains(needle)) {
                let end = line.find(['=', ':']).map(|index| index + 1).unwrap_or(0);
                format!("{} [redacted]", line[..end].trim_end())
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_help(items: &[CommandHelpItem]) -> String {
    let mut output = String::from("Commands:");
    for item in items {
        output.push_str(&format!("\n  /{:<12} {}", item.name, item.description));
    }
    output.push_str(
        "\n\nKeys: Enter submit · Ctrl+J newline · Tab focus · PageUp/Down scroll · End latest · Space expand · Ctrl+C cancel",
    );
    output
}

// ── Rendering ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RenderLine {
    text: String,
    style: Style,
}

pub fn view(frame: &mut Frame, state: &AppState) {
    let input_lines = input_height(state.terminal_size.0, state.terminal_size.1, &state.input);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_lines),
            Constraint::Length(1),
        ])
        .split(frame.area());
    render_transcript(frame, state, chunks[0]);
    render_input(frame, state, chunks[1]);
    render_status(frame, state, chunks[2]);
}

fn input_height(width: u16, height: u16, input: &InputState) -> u16 {
    if height <= 4 {
        return height.saturating_sub(1).max(1);
    }
    let max_height = (height.saturating_mul(3) / 10).clamp(3, 8);
    let inner_width = width.saturating_sub(2).max(1) as usize;
    let content_lines = input
        .text
        .split('\n')
        .map(|line| wrapped_line_count(line, inner_width))
        .sum::<usize>()
        .max(1);
    let cursor_line = input.cursor_visual_position(inner_width).0 + 1;
    let lines = content_lines.max(cursor_line) as u16;
    (lines + 2).clamp(3, max_height)
}

fn render_transcript(frame: &mut Frame, state: &AppState, area: Rect) {
    let lines = render_lines(state);
    let width = area.width.saturating_sub(2).max(1) as usize;
    let total = lines
        .iter()
        .map(|line| wrapped_line_count(&line.text, width))
        .sum::<usize>();
    let visible = area.height.saturating_sub(2).max(1) as usize;
    let max_top = total.saturating_sub(visible);
    let top = if state.scroll.follow_output {
        max_top
    } else {
        max_top.saturating_sub(state.scroll.offset_from_bottom)
    } as u16;

    let title = if state.scroll.follow_output {
        "Transcript".to_string()
    } else if state.scroll.unread_lines > 0 {
        format!("Transcript · {} new lines · End to follow", state.scroll.unread_lines)
    } else {
        "Transcript · browsing history".into()
    };
    let paragraph = Paragraph::new(
        lines
            .into_iter()
            .map(|line| Line::from(Span::styled(line.text, line.style)))
            .collect::<Vec<_>>(),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .scroll((top, 0))
    .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn render_lines(state: &AppState) -> Vec<RenderLine> {
    let mut lines = Vec::new();
    for (index, block) in state.transcript.iter().enumerate() {
        let selected = state.selection.selected_block == Some(index);
        let selection_style = |style: Style| {
            if selected {
                style.add_modifier(Modifier::REVERSED)
            } else {
                style
            }
        };
        match block {
            TranscriptBlock::User { text } => {
                lines.push(RenderLine {
                    text: "You".into(),
                    style: selection_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                });
                push_body_lines(&mut lines, text, Color::Cyan, selected);
            }
            TranscriptBlock::Assistant { text, streaming } => {
                lines.push(RenderLine {
                    text: if *streaming {
                        "Assistant · streaming"
                    } else {
                        "Assistant"
                    }
                    .into(),
                    style: selection_style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                });
                push_body_lines(&mut lines, text, Color::White, selected);
            }
            TranscriptBlock::Thinking { text, expanded } => {
                let line_count = text_line_count(text);
                lines.push(RenderLine {
                    text: if *expanded {
                        "▼ Thinking".into()
                    } else {
                        format!("▶ Thinking… {line_count} lines")
                    },
                    style: selection_style(Style::default().fg(Color::Magenta)),
                });
                if *expanded {
                    push_body_lines(&mut lines, text, Color::Magenta, selected);
                }
            }
            TranscriptBlock::Tool {
                name,
                arguments_display,
                stdout,
                stderr,
                state: tool_state,
                expanded,
                exit_code,
                ..
            } => {
                let exit = exit_code.map(|code| format!(" · exit {code}")).unwrap_or_default();
                lines.push(RenderLine {
                    text: if *expanded {
                        format!("▼ {name}  {}{exit}", tool_state.label())
                    } else {
                        format!("▶ {name}  {}{exit}", tool_state.label())
                    },
                    style: selection_style(tool_style(*tool_state)),
                });
                if *expanded {
                    lines.push(RenderLine {
                        text: "Arguments:".into(),
                        style: selection_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    });
                    push_indented_raw_lines(&mut lines, arguments_display, Color::Yellow, selected);
                    lines.push(RenderLine {
                        text: "stdout:".into(),
                        style: selection_style(Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD)),
                    });
                    push_indented_raw_lines(
                        &mut lines,
                        if stdout.is_empty() { "(empty)" } else { stdout },
                        Color::DarkGray,
                        selected,
                    );
                    lines.push(RenderLine {
                        text: "stderr:".into(),
                        style: selection_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    });
                    push_indented_raw_lines(
                        &mut lines,
                        if stderr.is_empty() { "(empty)" } else { stderr },
                        Color::Red,
                        selected,
                    );
                }
            }
            TranscriptBlock::Error { message } => {
                lines.push(RenderLine {
                    text: "Error".into(),
                    style: selection_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                });
                push_body_lines(&mut lines, message, Color::Red, selected);
            }
            TranscriptBlock::System { message } => {
                lines.push(RenderLine {
                    text: "System".into(),
                    style: selection_style(Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                });
                push_body_lines(&mut lines, message, Color::Blue, selected);
            }
        }
        lines.push(RenderLine {
            text: String::new(),
            style: Style::default(),
        });
    }
    lines
}

fn tool_style(state: ToolRunState) -> Style {
    match state {
        ToolRunState::Running => Style::default().fg(Color::Yellow),
        ToolRunState::Succeeded => Style::default().fg(Color::Green),
        ToolRunState::Failed | ToolRunState::TimedOut => Style::default().fg(Color::Red),
        ToolRunState::Aborted => Style::default().fg(Color::Yellow),
    }
}

fn push_body_lines(lines: &mut Vec<RenderLine>, text: &str, color: Color, selected: bool) {
    for line in text.split('\n') {
        lines.push(RenderLine {
            text: format!("  {line}"),
            style: if selected {
                Style::default().fg(color).add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(color)
            },
        });
    }
}

fn push_indented_raw_lines(lines: &mut Vec<RenderLine>, text: &str, color: Color, selected: bool) {
    for line in text.split('\n') {
        lines.push(RenderLine {
            text: format!("  {line}"),
            style: if selected {
                Style::default().fg(color).add_modifier(Modifier::REVERSED)
            } else {
                Style::default().fg(color)
            },
        });
    }
}

fn render_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let width = area.width.saturating_sub(2).max(1) as usize;
    let visible = area.height.saturating_sub(2).max(1) as usize;
    let (cursor_line, cursor_column) = state.input.cursor_visual_position(width);
    let viewport = state
        .input
        .viewport_line
        .min(cursor_line)
        .max(cursor_line.saturating_sub(visible.saturating_sub(1)));
    let active = state.focus == Focus::Input;
    let title = if state.activity != ActivityState::Idle {
        "Input · draft (Enter unavailable)"
    } else {
        "Input · Enter submit · Ctrl+J newline"
    };
    let paragraph = Paragraph::new(state.input.text.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(if active { Color::Cyan } else { Color::DarkGray })),
        )
        .scroll((viewport as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
    if active && area.width > 2 && area.height > 2 {
        let x = area.x + 1 + cursor_column.min(width.saturating_sub(1)) as u16;
        let y = area.y + 1 + cursor_line.saturating_sub(viewport) as u16;
        if y < area.bottom().saturating_sub(1) {
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), y));
        }
    }
}

fn render_status(frame: &mut Frame, state: &AppState, area: Rect) {
    let activity = match state.activity {
        ActivityState::Idle => "Ready",
        ActivityState::AgentRunning => "Agent running",
        ActivityState::AgentCancelling => "Agent cancelling",
        ActivityState::CommandRunning => "Command running",
        ActivityState::CommandCancelling => "Command cancelling",
    };
    let outcome: Option<String> = state.last_outcome.as_ref().map(|outcome| match outcome {
        RunOutcome::Completed => "Completed".into(),
        RunOutcome::Aborted => "Aborted".into(),
        RunOutcome::ProviderError(_) => "Provider error".into(),
        RunOutcome::ToolError(_) => "Tool error".into(),
    });
    let text = if area.width < 40 {
        if state.activity != ActivityState::Idle {
            format!("{activity} · Ctrl+C cancel")
        } else {
            format!("{activity} · Tab focus · Enter submit")
        }
    } else {
        let mut parts = vec![format!("{}/{}", state.provider, state.model), activity.into()];
        if let Some(outcome) = outcome {
            parts.push(format!("last: {outcome}"));
        }
        if let Some(tool) = state.transcript.iter().rev().find_map(|block| match block {
            TranscriptBlock::Tool {
                name,
                state: ToolRunState::Running,
                ..
            } => Some(name.as_str()),
            _ => None,
        }) {
            parts.push(format!("tool: {tool}"));
        }
        if !state.scroll.follow_output {
            parts.push(if state.scroll.unread_lines > 0 {
                format!("{} new lines · End latest", state.scroll.unread_lines)
            } else {
                "browsing history · End latest".into()
            });
        }
        parts.push(if state.focus == Focus::Input {
            "Input · Tab transcript · PageUp/Down scroll · Ctrl+C cancel".into()
        } else {
            "Transcript · Space expand · i input · End latest".into()
        });
        if !state.status.is_empty() && state.status != "Ready" && state.status != "Running" {
            parts.push(state.status.clone());
        }
        if let Some(error) = &state.error {
            parts.push(format!("Error: {error}"));
        }
        parts.join(" | ")
    };
    let style = if state.error.is_some() {
        Style::default().fg(Color::Red)
    } else if state.activity != ActivityState::Idle {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    };
    frame.render_widget(Paragraph::new(text).style(style), area);
}

fn wrapped_line_count(text: &str, width: usize) -> usize {
    let width = width.max(1);
    text.split('\n')
        .map(|line| {
            if line.is_empty() {
                return 1;
            }
            let mut lines = 1;
            let mut column = 0;
            for character in line.chars() {
                let character_width = char_width(character);
                if column > 0 && column + character_width > width {
                    lines += 1;
                    column = 0;
                }
                column += character_width;
            }
            lines.max(1)
        })
        .sum::<usize>()
        .max(1)
}

fn visual_position(text: &str, cursor: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let mut line = 0;
    let mut column = 0;
    for (index, character) in text.char_indices() {
        if index >= cursor {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 0;
            continue;
        }
        let character_width = char_width(character);
        if column > 0 && column + character_width > width {
            line += 1;
            column = 0;
        }
        column += character_width;
        if column >= width {
            line += 1;
            column = 0;
        }
    }
    (line, column)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::events::ProviderError;
    use crate::ai::types::Content;
    use crossterm::event::{KeyCode, KeyModifiers};
    use insta::assert_snapshot;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(character: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL)
    }

    fn run_started(state: &mut AppState) {
        state.update(Action::AgentEvent(AgentEvent::RunStarted { run_id: RunId::new(1) }));
    }

    fn tool_started(state: &mut AppState, id: &str) {
        state.update(Action::AgentEvent(AgentEvent::ToolStarted {
            run_id: RunId::new(1),
            tool_call_id: id.into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command":"ls"}),
        }));
    }

    fn render_state(state: &AppState) -> String {
        let backend = TestBackend::new(state.terminal_size.0, state.terminal_size.1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| view(frame, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn initial_state_uses_input_focus_and_follow_mode() {
        let state = AppState::new((80, 24));
        assert_eq!(state.focus, Focus::Input);
        assert!(state.scroll.follow_output);
        assert!(state.transcript.is_empty());
    }

    #[test]
    fn multiline_unicode_editor_preserves_boundaries() {
        let mut state = AppState::new((80, 24));
        for character in "你好\nworld".chars() {
            state.update(Action::KeyInput(key(KeyCode::Char(character))));
        }
        state.update(Action::KeyInput(key(KeyCode::Up)));
        state.update(Action::KeyInput(key(KeyCode::Home)));
        state.update(Action::KeyInput(key(KeyCode::Delete)));
        assert_eq!(state.input.text, "好\nworld");
        assert!(state.input.text.is_char_boundary(state.input.cursor_byte));
    }

    #[test]
    fn editor_supports_control_editing_and_paste() {
        let mut state = AppState::new((80, 24));
        state.update(Action::Paste("one two\n三".into()));
        state.update(Action::KeyInput(key(KeyCode::Up)));
        state.update(Action::KeyInput(ctrl('a')));
        state.update(Action::KeyInput(ctrl('k')));
        assert_eq!(state.input.text, "\n三");
        state.update(Action::KeyInput(ctrl('u')));
        assert_eq!(state.input.text, "\n三");
        state.update(Action::KeyInput(ctrl('w')));
        assert_eq!(state.input.text, "\n三");
    }

    #[test]
    fn enter_submits_original_multiline_prompt_and_history() {
        let mut state = AppState::new((80, 24));
        state.update(Action::Paste("first\nsecond".into()));
        let effects = state.update(Action::KeyInput(key(KeyCode::Enter)));
        assert!(matches!(effects.as_slice(), [Effect::SubmitInput(prompt)] if prompt == "first\nsecond"));
        assert!(state.transcript.is_empty());
        assert_eq!(state.history.entries, vec!["first\nsecond"]);
        let effects = state.update(Action::AgentPromptStarted {
            original: "first\nsecond".into(),
            expanded: "expanded".into(),
        });
        assert!(matches!(effects.as_slice(), [Effect::RunAgent(prompt)] if prompt == "expanded"));
        assert!(matches!(&state.transcript[0], TranscriptBlock::User { text } if text == "first\nsecond"));
    }

    #[test]
    fn empty_prompt_is_not_transcript_or_history() {
        let mut state = AppState::new((80, 24));
        state.update(Action::Paste(" \n ".into()));
        assert!(state.update(Action::Submit).is_empty());
        assert!(state.transcript.is_empty());
        assert!(state.history.entries.is_empty());
    }

    #[test]
    fn history_deduplicates_and_restores_draft() {
        let mut state = AppState::new((80, 24));
        for prompt in ["one", "one", "two"] {
            state.input.set_text(prompt.into());
            state.update(Action::Submit);
            state.activity = ActivityState::Idle;
        }
        assert_eq!(state.history.entries, vec!["one", "two"]);
        state.input.set_text("draft 中文".into());
        state.update(Action::KeyInput(key(KeyCode::Up)));
        assert_eq!(state.input.text, "two");
        state.update(Action::KeyInput(key(KeyCode::Up)));
        assert_eq!(state.input.text, "one");
        state.update(Action::KeyInput(key(KeyCode::Down)));
        assert_eq!(state.input.text, "two");
        state.update(Action::KeyInput(key(KeyCode::Down)));
        assert_eq!(state.input.text, "draft 中文");
        assert_eq!(state.history.entries, vec!["one", "two"]);
    }

    #[test]
    fn running_enter_is_rejected_but_draft_is_kept() {
        let mut state = AppState::new((80, 24));
        state.input.set_text("first".into());
        assert_eq!(state.update(Action::Submit).len(), 1);
        state.update(Action::AgentPromptStarted {
            original: "first".into(),
            expanded: "first".into(),
        });
        state.input.set_text("next".into());
        assert!(state.update(Action::KeyInput(key(KeyCode::Enter))).is_empty());
        assert_eq!(state.input.text, "next");
        assert!(state.status.contains("Enter unavailable"));
    }

    #[test]
    fn command_lifecycle_never_creates_user_transcript() {
        let mut state = AppState::new((80, 24));
        state.input.set_text("/session".into());
        assert!(matches!(
            state.update(Action::Submit).as_slice(),
            [Effect::SubmitInput(input)] if input == "/session"
        ));
        assert!(state.transcript.is_empty());
        state.update(Action::CommandStarted { name: "session".into() });
        assert_eq!(state.activity, ActivityState::CommandRunning);
        state.update(Action::CommandCompleted(CommandOutcome {
            result: Some(CommandResult::Message("info".into())),
            control: CommandControl::Continue,
        }));
        assert_eq!(state.activity, ActivityState::Idle);
        assert!(matches!(state.transcript.as_slice(), [TranscriptBlock::System { message }] if message == "info"));
    }

    #[test]
    fn command_ctrl_c_is_distinct_from_agent_cancellation() {
        let mut state = AppState::new((80, 24));
        state.update(Action::CommandStarted {
            name: "list-sessions".into(),
        });
        assert!(matches!(
            state.update(Action::KeyInput(ctrl('c'))).as_slice(),
            [Effect::CancelCommand]
        ));
        assert_eq!(state.activity, ActivityState::CommandCancelling);
        state.update(Action::CommandCancelled);
        state.update(Action::CommandCancelled);
        assert_eq!(
            state
                .transcript
                .iter()
                .filter(|block| matches!(block, TranscriptBlock::System { message } if message == "Command cancelled"))
                .count(),
            1
        );
        assert_eq!(state.activity, ActivityState::Idle);
    }

    #[test]
    fn ctrl_c_clears_idle_draft_and_cancels_running() {
        let mut state = AppState::new((80, 24));
        state.input.set_text("draft".into());
        assert!(state.update(Action::KeyInput(ctrl('c'))).is_empty());
        assert!(state.input.text.is_empty());
        state.activity = ActivityState::AgentRunning;
        assert!(matches!(
            state.update(Action::KeyInput(ctrl('c'))).as_slice(),
            [Effect::CancelAgent]
        ));
        assert_eq!(state.activity, ActivityState::AgentCancelling);
    }

    #[test]
    fn focus_routes_selection_without_toggling_non_expandable_blocks() {
        let mut state = AppState::new((80, 24));
        state.transcript.push(TranscriptBlock::User { text: "you".into() });
        state.transcript.push(TranscriptBlock::Thinking {
            text: "hmm".into(),
            expanded: false,
        });
        state
            .transcript
            .push(TranscriptBlock::tool("t".into(), "bash".into(), serde_json::json!({})));
        state.update(Action::KeyInput(key(KeyCode::Tab)));
        assert_eq!(state.focus, Focus::Transcript);
        state.selection.selected_block = Some(0);
        state.update(Action::KeyInput(key(KeyCode::Enter)));
        assert!(matches!(state.transcript[0], TranscriptBlock::User { .. }));
        state.selection.selected_block = Some(1);
        state.update(Action::KeyInput(key(KeyCode::Char(' '))));
        assert!(matches!(
            state.transcript[1],
            TranscriptBlock::Thinking { expanded: true, .. }
        ));
    }

    #[test]
    fn tool_expansion_is_independent_and_survives_output_and_finish() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        tool_started(&mut state, "a");
        tool_started(&mut state, "b");
        state.selection.selected_block = Some(0);
        state.focus = Focus::Transcript;
        state.update(Action::KeyInput(key(KeyCode::Char(' '))));
        state.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "a".into(),
            stream: ToolOutputStream::Stdout,
            chunk: "out".into(),
        }));
        state.update(Action::AgentEvent(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "a".into(),
            name: "bash".into(),
            result: AgentToolResult {
                exit_code: Some(0),
                content: vec![Content::Text { text: "out".into() }],
                ..Default::default()
            },
        }));
        assert!(matches!(
            state.transcript[0],
            TranscriptBlock::Tool {
                expanded: true,
                state: ToolRunState::Succeeded,
                ..
            }
        ));
        assert!(matches!(
            state.transcript[1],
            TranscriptBlock::Tool { expanded: false, .. }
        ));
    }

    #[test]
    fn thinking_is_collapsed_with_summary_and_expands() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        state.update(Action::AgentEvent(AgentEvent::ThinkingDelta {
            run_id: RunId::new(1),
            text: "a\nb".into(),
        }));
        let collapsed = render_state(&state);
        assert!(collapsed.contains("Thinking… 2 lines"));
        assert!(!collapsed.contains("  a"));
        state.focus = Focus::Transcript;
        state.selection.selected_block = Some(0);
        state.update(Action::KeyInput(key(KeyCode::Char(' '))));
        assert!(render_state(&state).contains("  a"));
    }

    #[test]
    fn assistant_deltas_merge_and_finish_without_empty_block() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        state.update(Action::AgentEvent(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "a".into(),
        }));
        state.update(Action::AgentEvent(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "b".into(),
        }));
        state.update(Action::AgentEvent(AgentEvent::RunFinished {
            run_id: RunId::new(1),
            stop_reason: StopReason::Stop,
        }));
        assert!(matches!(&state.transcript[0], TranscriptBlock::Assistant { text, streaming: false } if text == "ab"));
        assert_eq!(state.transcript.len(), 1);
    }

    #[test]
    fn one_thousand_deltas_stay_in_one_assistant_block() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        for _ in 0..1_000 {
            state.update(Action::AgentEvent(AgentEvent::TextDelta {
                run_id: RunId::new(1),
                text: "x".into(),
            }));
        }
        assert!(
            matches!(&state.transcript[..], [TranscriptBlock::Assistant { text, streaming: true }] if text.len() == 1_000)
        );
    }

    #[test]
    fn tool_streams_are_separate_and_unknown_id_is_safe() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        state.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "orphan".into(),
            stream: ToolOutputStream::Stdout,
            chunk: "out".into(),
        }));
        state.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "orphan".into(),
            stream: ToolOutputStream::Stderr,
            chunk: "err".into(),
        }));
        assert!(
            matches!(&state.transcript[0], TranscriptBlock::Tool { stdout, stderr, name, .. } if stdout == "out" && stderr == "err" && name == "unknown")
        );
    }

    #[test]
    fn expanded_tool_render_has_arguments_and_separate_stream_labels() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        tool_started(&mut state, "render");
        state.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "render".into(),
            stream: ToolOutputStream::Stdout,
            chunk: "out".into(),
        }));
        state.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "render".into(),
            stream: ToolOutputStream::Stderr,
            chunk: "err".into(),
        }));
        state.focus = Focus::Transcript;
        state.selection.selected_block = Some(0);
        state.update(Action::KeyInput(key(KeyCode::Char(' '))));
        let rendered = render_state(&state);
        assert!(rendered.contains("Arguments:"));
        assert!(rendered.contains("stdout:"));
        assert!(rendered.contains("stderr:"));
        assert!(rendered.contains("out"));
        assert!(rendered.contains("err"));
    }

    #[test]
    fn tool_states_include_exit_code_timeout_and_abort() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        for (id, result, expected) in [
            (
                "failed",
                AgentToolResult {
                    is_error: true,
                    exit_code: Some(42),
                    ..Default::default()
                },
                ToolRunState::Failed,
            ),
            (
                "timeout",
                AgentToolResult {
                    is_error: true,
                    timed_out: true,
                    ..Default::default()
                },
                ToolRunState::TimedOut,
            ),
            (
                "abort",
                AgentToolResult {
                    is_error: true,
                    aborted: true,
                    ..Default::default()
                },
                ToolRunState::Aborted,
            ),
        ] {
            tool_started(&mut state, id);
            state.update(Action::AgentEvent(AgentEvent::ToolFinished {
                run_id: RunId::new(1),
                tool_call_id: id.into(),
                name: "bash".into(),
                result,
            }));
            assert!(matches!(state.transcript.last(), Some(TranscriptBlock::Tool { state, .. }) if *state == expected));
        }
    }

    #[test]
    fn follow_mode_and_unread_counter() {
        let mut state = AppState::new((80, 24));
        for i in 0..30 {
            state.transcript.push(TranscriptBlock::System {
                message: format!("line {i}"),
            });
        }
        state.update(Action::KeyInput(key(KeyCode::PageUp)));
        assert!(!state.scroll.follow_output);
        state.update(Action::AgentEvent(AgentEvent::RunStarted { run_id: RunId::new(1) }));
        state.update(Action::AgentEvent(AgentEvent::TextDelta {
            run_id: RunId::new(1),
            text: "new output".into(),
        }));
        assert!(state.scroll.unread_lines > 0);
        state.update(Action::KeyInput(key(KeyCode::End)));
        assert!(state.scroll.follow_output);
        assert_eq!(state.scroll.unread_lines, 0);
    }

    #[test]
    fn scroll_clamps_on_resize_and_empty_transcript() {
        let mut state = AppState::new((80, 24));
        state.scroll_to_start();
        state.update(Action::Resize(20, 8));
        assert!(state.scroll.offset_from_bottom <= state.total_transcript_lines());
        state.update(Action::Resize(20, 3));
        state.update(Action::KeyInput(key(KeyCode::PageDown)));
        assert!(state.scroll.offset_from_bottom <= state.total_transcript_lines());
        let _ = render_state(&state);
    }

    #[test]
    fn output_is_utf8_safe_and_has_one_truncation_marker() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        tool_started(&mut state, "large");
        // This is deliberately larger than the UI limit: it exercises the
        // same reducer path with a 1 MiB burst without retaining that burst.
        let chunk = "界".repeat(1024 * 1024);
        for _ in 0..1 {
            state.update(Action::AgentEvent(AgentEvent::ToolOutput {
                run_id: RunId::new(1),
                tool_call_id: "large".into(),
                stream: ToolOutputStream::Stdout,
                chunk: chunk.clone(),
            }));
        }
        let TranscriptBlock::Tool { stdout, .. } = &state.transcript[0] else {
            panic!("tool")
        };
        assert!(stdout.is_char_boundary(stdout.len()));
        assert_eq!(stdout.matches(OUTPUT_TRUNCATION_MARKER).count(), 1);
    }

    #[test]
    fn provider_error_is_separate_and_sanitized() {
        let mut state = AppState::new((80, 24));
        run_started(&mut state);
        state.update(Action::AgentEvent(AgentEvent::ProviderError {
            run_id: RunId::new(1),
            error: ProviderError {
                reason: StopReason::Error,
                message: "Authorization: Bearer secret".into(),
            },
        }));
        assert!(matches!(&state.transcript[0], TranscriptBlock::Error { message } if !message.contains("secret")));
        assert!(!render_state(&state).contains("secret"));
    }

    #[test]
    fn test_backend_renders_all_requested_sizes_and_unicode() {
        for size in [(120, 40), (80, 24), (60, 18), (40, 12), (20, 8)] {
            let mut state = AppState::new(size);
            state.input.set_text("你好\nworld".into());
            state.transcript.push(TranscriptBlock::User {
                text: "多行 prompt\n第二行".into(),
            });
            state.transcript.push(TranscriptBlock::Assistant {
                text: "response".into(),
                streaming: true,
            });
            assert!(!render_state(&state).is_empty());
        }
    }

    #[test]
    fn snapshots_cover_stable_idle_and_tool_views() {
        let state = AppState::new((80, 24));
        assert_snapshot!("idle_v2", render_state(&state));
        let mut tool = AppState::new((80, 24));
        run_started(&mut tool);
        tool_started(&mut tool, "normalized");
        assert_snapshot!("tool_collapsed_v2", render_state(&tool));
        tool.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "normalized".into(),
            stream: ToolOutputStream::Stdout,
            chunk: "listed file".into(),
        }));
        tool.update(Action::AgentEvent(AgentEvent::ToolOutput {
            run_id: RunId::new(1),
            tool_call_id: "normalized".into(),
            stream: ToolOutputStream::Stderr,
            chunk: "warning".into(),
        }));
        tool.update(Action::AgentEvent(AgentEvent::ToolFinished {
            run_id: RunId::new(1),
            tool_call_id: "normalized".into(),
            name: "bash".into(),
            result: AgentToolResult {
                exit_code: Some(0),
                ..Default::default()
            },
        }));
        tool.focus = Focus::Transcript;
        tool.selection.selected_block = Some(0);
        tool.update(Action::KeyInput(key(KeyCode::Char(' '))));
        assert_snapshot!("tool_expanded_v2", render_state(&tool));
    }
}
