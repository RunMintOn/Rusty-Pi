//! Command registry — extensible slash-command system for the REPL.
//!
//! Provides [`Command`] trait, [`CommandRegistry`], and built-in commands
//! (`/help`, `/exit`, `/quit`).

use anyhow::Result;
use crate::coding_agent::prompt_session::PromptSession;

/// Outcome of dispatching a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// Input does not start with `/` — not a command.
    NotACommand,
    /// A command was found and executed; continue the REPL loop.
    Handled,
    /// The command requested REPL exit.
    Exit,
}

/// A single slash command.
pub trait Command: Send + Sync {
    /// Command name (without the `/` prefix), e.g. `"help"`.
    fn name(&self) -> &str;

    /// One-line description shown in `/help`.
    fn description(&self) -> &str;

    /// Execute the command with optional arguments and mutable access to the session.
    fn execute(&self, args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome>;
}

/// Registry of available slash commands.
///
/// Usage:
/// ```ignore
/// let mut registry = CommandRegistry::new();
/// registry.register(Box::new(HelpCommand::new(vec![
///     ("exit", "Exit the REPL"),
/// ])));
/// registry.register(Box::new(ExitCommand));
///
/// match registry.dispatch("/exit")? {
///     DispatchOutcome::Exit => break,
///     _ => continue,
/// }
/// ```
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
}

impl CommandRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
        }
    }

    /// Register a command.
    pub fn register(&mut self, cmd: Box<dyn Command>) {
        // Replace existing command with the same name
        if let Some(pos) = self.commands.iter().position(|c| c.name() == cmd.name()) {
            self.commands[pos] = cmd;
        } else {
            self.commands.push(cmd);
        }
    }

    /// Try to dispatch an input line as a command.
    ///
    /// `session` is passed through to `Command::execute` so commands can
    /// interact with the agent, session, provider, etc.
    ///
    /// Returns:
    /// - `Ok(NotACommand)` — input does not start with `/`.
    /// - `Ok(Handled)` — command was found and executed.
    /// - `Ok(Exit)` — exit command was executed.
    /// - `Err(e)` — command execution failed.
    pub fn dispatch(&self, input: &str, session: &mut PromptSession) -> Result<DispatchOutcome> {
        let trimmed = input.trim();
        if !trimmed.starts_with('/') {
            return Ok(DispatchOutcome::NotACommand);
        }

        let parts: Vec<&str> = trimmed[1..].splitn(2, ' ').collect();
        let cmd_name = parts[0];
        let args: Vec<&str> = parts
            .get(1)
            .map(|s| s.split_whitespace().collect())
            .unwrap_or_default();

        // Look for exact match
        for cmd in &self.commands {
            if cmd.name() == cmd_name {
                return cmd.execute(&args, session);
            }
        }

        // Unknown command — still mark as handled so we don't send it to the agent
        eprintln!("[error] Unknown command '/{}'. Type '/help' for available commands.", cmd_name);
        Ok(DispatchOutcome::Handled)
    }

    /// Generate help text listing all registered commands.
    pub fn help_text(&self) -> String {
        let mut out = String::new();
        out.push_str("\n  Commands:\n");
        for cmd in &self.commands {
            use std::fmt::Write;
            let _ = write!(out, "    /{:<12} {}\n", cmd.name(), cmd.description());
        }
        out.push_str("\n  Tips:\n");
        out.push_str("    - Up/down arrows navigate command history\n");
        out.push_str("    - Ctrl+C at prompt exits\n");
        out.push_str("    - Ctrl+C during agent run aborts the current round\n");
        out.push_str("    - Type any text to chat with the agent\n");
        out
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Built-in commands ────────────────────────────────────────────────────

/// `/help` — list available commands.
pub struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "Show this help message"
    }

    fn execute(&self, _args: &[&str], _session: &mut PromptSession) -> Result<DispatchOutcome> {
        // This is a placeholder — the REPL handles `/help` before dispatch.
        // This command exists so `/help` appears in the registry listing.
        Ok(DispatchOutcome::Handled)
    }
}

/// `/exit` / `/quit` — exit the REPL.
pub struct ExitCommand;

impl Command for ExitCommand {
    fn name(&self) -> &str {
        "exit"
    }

    fn description(&self) -> &str {
        "Exit the REPL"
    }

    fn execute(&self, _args: &[&str], _session: &mut PromptSession) -> Result<DispatchOutcome> {
        Ok(DispatchOutcome::Exit)
    }
}

/// `/quit` — alias for `/exit`.
pub struct QuitCommand;

impl Command for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }

    fn description(&self) -> &str {
        "Exit the REPL"
    }

    fn execute(&self, _args: &[&str], _session: &mut PromptSession) -> Result<DispatchOutcome> {
        Ok(DispatchOutcome::Exit)
    }
}

// ── Interactive commands (Ticket 21) ────────────────────────────────────

use crate::coding_agent::picker::Picker;

/// `/model` — switch model via interactive selector.
pub struct ModelCommand {
    picker: Box<dyn Picker + Send + Sync>,
}

impl ModelCommand {
    pub fn new(picker: Box<dyn Picker + Send + Sync>) -> Self {
        Self { picker }
    }
}

impl Command for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }

    fn description(&self) -> &str {
        "Switch model (interactive selector)"
    }

    fn execute(&self, _args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome> {
        // Gather model IDs from the provider
        let models = {
            let agent = session.agent();
            agent.list_models().into_iter().map(|m| m.id.to_string()).collect::<Vec<_>>()
        };

        if models.is_empty() {
            // Provider doesn't support model listing
            let current = {
                let agent = session.agent();
                agent.model().id.to_string()
            };
            println!("Current model: {}. This provider doesn't support runtime model switching.", current);
            return Ok(DispatchOutcome::Handled);
        }

        let selected = self.picker.select("Select model:", models)?;
        let current_id = {
            let agent = session.agent();
            agent.model().id.to_string()
        };

        if selected == current_id {
            println!("Already using {}", selected);
        } else {
            // Find the Model struct matching the selected ID
            let model = {
                let agent = session.agent();
                agent.list_models().into_iter().find(|m| m.id == selected).cloned()
            };
            if let Some(m) = model {
                session.switch_model(m);
                println!("✓ Switched to {}", selected);
            } else {
                println!("Model '{}' not found", selected);
            }
        }
        Ok(DispatchOutcome::Handled)
    }
}

/// `/context` — inject a file into the system prompt.
pub struct ContextCommand {
    picker: Box<dyn Picker + Send + Sync>,
}

impl ContextCommand {
    pub fn new(picker: Box<dyn Picker + Send + Sync>) -> Self {
        Self { picker }
    }
}

impl Command for ContextCommand {
    fn name(&self) -> &str {
        "context"
    }

    fn description(&self) -> &str {
        "Inject a file into the system prompt"
    }

    fn execute(&self, args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome> {
        let path_str = if args.is_empty() {
            self.picker.text("File path:", None, Some("Path to file to inject"))?
        } else {
            args.join(" ")
        };

        let path = std::path::PathBuf::from(&path_str);
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path_str, e))?;
        let size_kb = content.len() / 1024;
        session.add_context_file(path, content);
        println!("✓ Added {} ({}KB) to system prompt", path_str, size_kb);
        Ok(DispatchOutcome::Handled)
    }
}

#[cfg(test)]
mod ticket21_tests {
    use super::*;
    use crate::coding_agent::picker::MockPicker;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use std::path::PathBuf;

    fn mock_session() -> PromptSession {
        let provider = MockProvider::text("mock");
        let model = Model { id: "mock", api: "mock" };
        PromptSession::new(
            Box::new(provider),
            model,
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
    fn model_command_shows_current_when_no_models() {
        let picker = Box::new(MockPicker::new(vec![], vec![]));
        let cmd = ModelCommand::new(picker);
        let mut session = mock_session();
        // Provider has no models (MockProvider returns empty list)
        let result = cmd.execute(&[], &mut session);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), DispatchOutcome::Handled);
    }

    #[test]
    fn context_command_requires_valid_file() {
        let picker = Box::new(MockPicker::new(vec![], vec![]));
        let cmd = ContextCommand::new(picker);
        let mut session = mock_session();
        // No file path provided and picker has no text values
        let result = cmd.execute(&[], &mut session);
        assert!(result.is_err());
    }

    #[test]
    fn context_command_with_arg_uses_it_as_path() {
        let picker = Box::new(MockPicker::new(vec![], vec![]));
        let cmd = ContextCommand::new(picker);
        let mut session = mock_session();
        // Pass a non-existent file path as arg — should error
        let result = cmd.execute(&["/nonexistent/path"], &mut session);
        assert!(result.is_err());
    }

    #[test]
    fn model_command_dispatch_via_registry() {
        let picker = Box::new(MockPicker::new(
            vec!["model-123".into()],
            vec![],
        ));
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(ModelCommand::new(picker)));
        let mut session = mock_session();
        let outcome = registry.dispatch("/model", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Handled);
    }
}

// ── Session display commands (Ticket 22) ────────────────────────────────

use crate::agent::session::types::SessionTreeEntry;
use crate::ai::types::{AgentMessage, AssistantContent, MessageContent};
use crate::format::{OutputFormatter, SessionInfo, SessionSummary};

/// `/session` — display current session information.
pub struct SessionCommand;

impl Command for SessionCommand {
    fn name(&self) -> &str {
        "session"
    }

    fn description(&self) -> &str {
        "Show current session information"
    }

    fn execute(&self, _args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome> {
        let rt = tokio::runtime::Handle::current();
        let s = session.session();
        let meta = rt.block_on(s.get_metadata());
        let (total, _user, _assistant, _tool) = rt.block_on(s.count_messages());
        let model = rt.block_on(s.derive_model()).unwrap_or_default();

        let info = SessionInfo {
            id: meta.id,
            model,
            msg_count: total,
            cwd: meta.cwd,
        };

        let fmt = OutputFormatter::new();
        println!("{}", fmt.session_info(&info));
        Ok(DispatchOutcome::Handled)
    }
}

/// `/tree` — display session entry tree.
pub struct TreeCommand;

impl Command for TreeCommand {
    fn name(&self) -> &str {
        "tree"
    }

    fn description(&self) -> &str {
        "Show session tree structure"
    }

    fn execute(&self, _args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome> {
        let rt = tokio::runtime::Handle::current();
        let s = session.session();
        let entries = rt.block_on(s.get_branch(None))
            .map_err(|e| anyhow::anyhow!("Failed to get session branch: {}", e))?;

        if entries.is_empty() {
            println!("(empty session)");
            return Ok(DispatchOutcome::Handled);
        }

        // Simple indented tree output
        for entry in &entries {
            let indent = "  ".to_string();
            let label = match entry {
                SessionTreeEntry::Message(m) => {
                    match &m.message {
                        AgentMessage::User(u) => {
                            let preview = match &u.content {
                                MessageContent::Text(t) => {
                                    if t.len() > 60 { format!("{}...", &t[..60]) } else { t.clone() }
                                },
                                _ => "(non-text)".into(),
                            };
                            format!("user: {}", preview)
                        },
                        AgentMessage::Assistant(a) => {
                            let preview = a.content.first().map(|c| match c {
                                AssistantContent::Text { text } => {
                                    if text.len() > 60 { format!("{}...", &text[..60]) } else { text.clone() }
                                },
                                _ => "(tool call)".into(),
                            }).unwrap_or_default();
                            format!("assistant: {}", preview)
                        },
                        AgentMessage::ToolResult(t) => {
                            format!("tool: {}", t.tool_name)
                        },
                        _ => "(other)".into(),
                    }
                },
                _ => format!("{:?}", entry.entry_type()),
            };
            println!("{}{}", indent, label);
        }
        Ok(DispatchOutcome::Handled)
    }
}

/// `/list-sessions` — list all saved sessions.
pub struct ListSessionsCommand;

impl Command for ListSessionsCommand {
    fn name(&self) -> &str {
        "list-sessions"
    }

    fn description(&self) -> &str {
        "List all saved sessions"
    }

    fn execute(&self, _args: &[&str], session: &mut PromptSession) -> Result<DispatchOutcome> {
        let sessions_dir = session.agent_dir().join("sessions");

        if !sessions_dir.exists() {
            println!("No sessions directory found at: {}", sessions_dir.display());
            return Ok(DispatchOutcome::Handled);
        }

        let mut summaries: Vec<SessionSummary> = Vec::new();
        let dir_entries = match std::fs::read_dir(&sessions_dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("[error] Cannot read sessions directory: {}", e);
                return Ok(DispatchOutcome::Handled);
            }
        };

        let rt = tokio::runtime::Handle::current();
        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Try to open and read header
            let path_str = path.to_string_lossy().to_string();
            use crate::agent::session::storage::SessionStorage;
            if let Ok(storage) = rt.block_on(
                crate::agent::session::jsonl::JsonlSessionStorage::open(path_str)
            ) {
                let meta = rt.block_on(storage.get_metadata());
                summaries.push(SessionSummary {
                    id: meta.id,
                    model: String::new(),
                    msg_count: 0,
                    created: meta.created_at,
                });
            }
        }

        // Sort by created_at descending
        summaries.sort_by(|a, b| b.created.cmp(&a.created));

        let fmt = OutputFormatter::new();
        println!("{}", fmt.session_list(&summaries));
        Ok(DispatchOutcome::Handled)
    }
}

#[cfg(test)]
mod ticket22_tests {
    use super::*;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use std::path::PathBuf;

    fn mock_session() -> PromptSession {
        let provider = MockProvider::text("mock");
        let model = Model { id: "mock", api: "mock" };
        PromptSession::new(
            Box::new(provider),
            model,
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

    fn run_in_runtime<F>(f: F)
    where
        F: FnOnce() + std::panic::UnwindSafe,
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        f();
    }

    #[test]
    fn session_command_returns_handled() {
        run_in_runtime(|| {
            let cmd = SessionCommand;
            let mut session = mock_session();
            let result = cmd.execute(&[], &mut session);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), DispatchOutcome::Handled);
        });
    }

    #[test]
    fn tree_command_returns_handled() {
        run_in_runtime(|| {
            let cmd = TreeCommand;
            let mut session = mock_session();
            let result = cmd.execute(&[], &mut session);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), DispatchOutcome::Handled);
        });
    }

    #[test]
    fn list_sessions_command_returns_handled() {
        run_in_runtime(|| {
            let cmd = ListSessionsCommand;
            let mut session = mock_session();
            let result = cmd.execute(&[], &mut session);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), DispatchOutcome::Handled);
        });
    }

    #[test]
    fn session_command_dispatch_via_registry() {
        run_in_runtime(|| {
            let mut registry = CommandRegistry::new();
            registry.register(Box::new(SessionCommand));
            let mut session = mock_session();
            let outcome = registry.dispatch("/session", &mut session).unwrap();
            assert_eq!(outcome, DispatchOutcome::Handled);
        });
    }

    #[test]
    fn list_sessions_dispatch_via_registry() {
        run_in_runtime(|| {
            let mut registry = CommandRegistry::new();
            registry.register(Box::new(ListSessionsCommand));
            let mut session = mock_session();
            let outcome = registry.dispatch("/list-sessions", &mut session).unwrap();
            assert_eq!(outcome, DispatchOutcome::Handled);
        });
    }
}

// ── LineReader trait (for testable REPL) ────────────────────────────────

/// Abstract line reader used by the REPL.
///
/// Production: backed by `rustyline::DefaultEditor`.
/// Testing: backed by [`MockLineReader`] with pre-defined inputs.
pub trait LineReader {
    /// Read one line of input.
    fn readline(&mut self, prompt: &str) -> Result<String, rustyline::error::ReadlineError>;

    /// Add a line to the history.
    fn add_history_entry(&mut self, line: &str);

    /// Persist history to disk (no-op for mock readers).
    fn save_history(&mut self, _path: &std::path::Path) -> std::result::Result<(), rustyline::error::ReadlineError> {
        Ok(())
    }
}

/// Mock line reader for testing the REPL loop.
///
/// Returns lines in order, then `ReadlineError::Eof` when exhausted.
pub struct MockLineReader {
    pub lines: Vec<String>,
    pub history: Vec<String>,
    idx: usize,
}

impl MockLineReader {
    pub fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            history: Vec::new(),
            idx: 0,
        }
    }
}

impl LineReader for MockLineReader {
    fn readline(&mut self, _prompt: &str) -> Result<String, rustyline::error::ReadlineError> {
        if self.idx < self.lines.len() {
            let line = self.lines[self.idx].clone();
            self.idx += 1;
            Ok(line)
        } else {
            Err(rustyline::error::ReadlineError::Eof)
        }
    }

    fn add_history_entry(&mut self, line: &str) {
        self.history.push(line.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use std::path::PathBuf;

    /// Create a minimal PromptSession for testing command dispatch.
    fn mock_session() -> PromptSession {
        let provider = MockProvider::text("mock");
        let model = Model { id: "mock", api: "mock" };
        PromptSession::new(
            Box::new(provider),
            model,
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

    // ── CommandRegistry basics ─────────────────────────────────────────

    #[test]
    fn registry_empty_dispatch_unknown() {
        let registry = CommandRegistry::new();
        let mut session = mock_session();
        let outcome = registry.dispatch("/unknown", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Handled);
    }

    #[test]
    fn registry_non_command_passthrough() {
        let registry = CommandRegistry::new();
        let mut session = mock_session();
        let outcome = registry.dispatch("hello", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::NotACommand);
    }

    #[test]
    fn registry_empty_line_passthrough() {
        let registry = CommandRegistry::new();
        let mut session = mock_session();
        let outcome = registry.dispatch("", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::NotACommand);
    }

    #[test]
    fn registry_exit_command() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(ExitCommand));
        let mut session = mock_session();
        let outcome = registry.dispatch("/exit", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Exit);
    }

    #[test]
    fn registry_quit_command() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(QuitCommand));
        let mut session = mock_session();
        let outcome = registry.dispatch("/quit", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Exit);
    }

    #[test]
    fn registry_help_command_listed() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(HelpCommand));
        registry.register(Box::new(ExitCommand));
        let text = registry.help_text();
        assert!(text.contains("/exit"), "help should list /exit");
        assert!(text.contains("/help"), "help should list /help");
        assert!(text.contains("Commands:"), "help should have header");
    }

    #[test]
    fn registry_help_dispatch() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(HelpCommand));
        let mut session = mock_session();
        let outcome = registry.dispatch("/help", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Handled);
    }

    #[test]
    fn registry_replaces_existing_command() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(ExitCommand));
        registry.register(Box::new(ExitCommand)); // replace
        let mut session = mock_session();
        let outcome = registry.dispatch("/exit", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Exit);
    }

    #[test]
    fn registry_command_with_args_passthrough() {
        let mut registry = CommandRegistry::new();
        registry.register(Box::new(ExitCommand));
        let mut session = mock_session();
        // exit ignores args
        let outcome = registry.dispatch("/exit now", &mut session).unwrap();
        assert_eq!(outcome, DispatchOutcome::Exit);
    }

    // ── MockLineReader ─────────────────────────────────────────────────

    #[test]
    fn mock_reader_returns_lines_in_order() {
        let mut reader = MockLineReader::new(vec![
            "first".into(),
            "second".into(),
        ]);
        assert_eq!(reader.readline("> ").unwrap(), "first");
        assert_eq!(reader.readline("> ").unwrap(), "second");
    }

    #[test]
    fn mock_reader_returns_eof_when_exhausted() {
        let mut reader = MockLineReader::new(vec!["only".into()]);
        let _ = reader.readline("> ").unwrap();
        let err = reader.readline("> ").unwrap_err();
        assert!(matches!(err, rustyline::error::ReadlineError::Eof));
    }

    #[test]
    fn mock_reader_tracks_history() {
        let mut reader = MockLineReader::new(vec!["a".into(), "b".into()]);
        reader.add_history_entry("a");
        reader.add_history_entry("b");
        assert_eq!(reader.history, vec!["a", "b"]);
    }

    // ── Help text content ──────────────────────────────────────────────

    #[test]
    fn help_text_includes_tips() {
        let registry = CommandRegistry::new();
        let text = registry.help_text();
        assert!(text.contains("Ctrl+C"));
        assert!(text.contains("Up/down arrows"));
    }
}
