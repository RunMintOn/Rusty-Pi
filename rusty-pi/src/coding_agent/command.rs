//! Command registry — extensible slash-command system for the REPL.
//!
//! Provides [`Command`] trait, [`CommandRegistry`], and built-in commands
//! (`/help`, `/exit`, `/quit`).

use crate::coding_agent::prompt_session::PromptSession;
use crate::format::OutputFormatter;
use anyhow::Result;
use std::sync::OnceLock;

/// Run an async future to completion in a dedicated blocking runtime.
///
/// Tokio does not allow entering ANY runtime from within an existing runtime
/// context. This function works around that by:
///
/// 1. Initializing a separate `tokio::Runtime` on a fresh thread (the
///    `OnceLock` init thread has no tokio context).
/// 2. Running the future on yet another fresh thread via `std::thread::scope`,
///    which also has no tokio context, so `Runtime::block_on` succeeds.
///
/// The scope also lets the future borrow non-`'static` references (e.g. `&Session`).
fn block_on<'a, F, T>(f: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'a,
    T: 'a + Send,
{
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    // Initialize on a fresh thread — `Runtime::new()` panics if called from
    // within another runtime.
    RT.get_or_init(|| {
        std::thread::spawn(|| tokio::runtime::Runtime::new().expect("failed to create blocking runtime"))
            .join()
            .expect("blocking runtime init thread panicked")
    });
    let rt = RT.get().expect("blocking runtime not initialized");
    // Run on a scoped thread — it has no tokio context, so `rt.block_on` can
    // freely enter the separate runtime.
    std::thread::scope(|s| s.spawn(|| rt.block_on(f)).join().expect("blocking future panicked"))
}

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
        Self { commands: Vec::new() }
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
        let args: Vec<&str> = parts.get(1).map(|s| s.split_whitespace().collect()).unwrap_or_default();

        // Handle /help directly so it works even when HelpCommand is a stub
        if cmd_name == "help" {
            print!("{}", self.help_text());
            return Ok(DispatchOutcome::Handled);
        }

        // Look for exact match
        for cmd in &self.commands {
            if cmd.name() == cmd_name {
                return cmd.execute(&args, session);
            }
        }

        // Unknown command — still mark as handled so we don't send it to the agent
        let fmt = OutputFormatter::new();
        eprintln!(
            "{}",
            fmt.error(&format!(
                "Unknown command '/{}'. Type '/help' for available commands.",
                cmd_name
            ))
        );
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
///
/// This command is registered so it appears in `/help` listing.
/// The actual help output is produced by [`CommandRegistry::dispatch`]
/// when it matches `cmd_name == "help"`.
pub struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &str {
        "help"
    }

    fn description(&self) -> &str {
        "Show this help message"
    }

    fn execute(&self, _args: &[&str], _session: &mut PromptSession) -> Result<DispatchOutcome> {
        // dispatch() handles /help before reaching this stub.
        // This execute() exists only to make the command appear in the registry listing.
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
            agent
                .list_models()
                .into_iter()
                .map(|m| m.id.to_string())
                .collect::<Vec<_>>()
        };

        if models.is_empty() {
            // Provider doesn't support model listing
            let current = {
                let agent = session.agent();
                agent.model().id.to_string()
            };
            println!(
                "Current model: {}. This provider doesn't support runtime model switching.",
                current
            );
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
        let content = std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path_str, e))?;
        let size_kb = content.len() / 1024;
        session.add_context_file(path, content);
        println!("✓ Added {} ({}KB) to system prompt", path_str, size_kb);
        Ok(DispatchOutcome::Handled)
    }
}

#[cfg(test)]
mod ticket21_tests {
    use super::*;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use crate::coding_agent::picker::MockPicker;
    use std::path::PathBuf;

    fn mock_session() -> PromptSession {
        let provider = MockProvider::text("mock");
        let model = Model {
            id: "mock",
            api: "mock",
        };
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
        let picker = Box::new(MockPicker::new(vec!["model-123".into()], vec![]));
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
use crate::format::{SessionInfo, SessionSummary};

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
        let s = session.session();
        let meta = block_on(s.get_metadata());
        let (total, _user, _assistant, _tool) = block_on(s.count_messages());
        let model = block_on(s.derive_model()).unwrap_or_default();

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
        let s = session.session();
        let entries = block_on(s.get_entries());

        if entries.is_empty() {
            println!("(empty session)");
            return Ok(DispatchOutcome::Handled);
        }

        // Build parent → children map
        use std::collections::{HashMap, HashSet};
        let all_ids: HashSet<&str> = entries.iter().map(|e| e.id()).collect();
        let mut children: HashMap<Option<String>, Vec<&SessionTreeEntry>> = HashMap::new();
        for entry in &entries {
            let pid = entry.parent_id().map(|s| s.to_string());
            children.entry(pid).or_default().push(entry);
        }

        // Find roots: entries whose parent_id is None or points outside the set
        let roots: Vec<&SessionTreeEntry> = entries
            .iter()
            .filter(|e| e.parent_id().map_or(true, |pid| !all_ids.contains(pid)))
            .collect();

        fn label_for_entry(entry: &SessionTreeEntry) -> String {
            match entry {
                SessionTreeEntry::Message(m) => match &m.message {
                    AgentMessage::User(u) => {
                        let preview = match &u.content {
                            MessageContent::Text(t) => {
                                if t.len() > 60 {
                                    format!("{}...", &t[..60])
                                } else {
                                    t.clone()
                                }
                            }
                            _ => "(non-text)".into(),
                        };
                        format!("user: {}", preview)
                    }
                    AgentMessage::Assistant(a) => {
                        let preview = a
                            .content
                            .first()
                            .map(|c| match c {
                                AssistantContent::Text { text } => {
                                    if text.len() > 60 {
                                        format!("{}...", &text[..60])
                                    } else {
                                        text.clone()
                                    }
                                }
                                _ => "(tool call)".into(),
                            })
                            .unwrap_or_default();
                        format!("assistant: {}", preview)
                    }
                    AgentMessage::ToolResult(t) => {
                        format!("tool: {}", t.tool_name)
                    }
                    _ => "(other)".into(),
                },
                _ => format!("{:?}", entry.entry_type()),
            }
        }

        fn render_children(
            out: &mut String,
            parent_id: &str,
            children: &HashMap<Option<String>, Vec<&SessionTreeEntry>>,
            prefix: &str,
        ) {
            let kid_key = Some(parent_id.to_string());
            if let Some(kids) = children.get(&kid_key) {
                let total = kids.len();
                for (i, kid) in kids.iter().enumerate() {
                    let is_last = i == total - 1;
                    let connector = if is_last { "└── " } else { "├── " };
                    let continuation = if is_last { "    " } else { "│   " };
                    let full_prefix = format!("{}{}", prefix, connector);
                    let next_prefix = format!("{}{}", prefix, continuation);
                    out.push_str(&format!("{}{}\n", full_prefix, label_for_entry(kid)));
                    render_children(out, kid.id(), children, &next_prefix);
                }
            }
        }

        let mut output = String::new();
        let total_roots = roots.len();
        for (i, root) in roots.iter().enumerate() {
            let is_last = i == total_roots - 1;
            let connector = if is_last { "└── " } else { "├── " };
            let prefix = if is_last { "    " } else { "│   " };
            output.push_str(&format!("{}{}\n", connector, label_for_entry(root)));
            render_children(&mut output, root.id(), &children, prefix);
        }
        print!("{}", output);
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
        use crate::agent::session::jsonl::JsonlSessionStorage;
        use crate::agent::session::storage::SessionStorage;
        use crate::ai::types::AgentMessage;

        let sessions_dir = session.agent_dir().join("sessions");

        if !sessions_dir.exists() {
            println!("No sessions directory found at: {}", sessions_dir.display());
            return Ok(DispatchOutcome::Handled);
        }

        let mut summaries: Vec<SessionSummary> = Vec::new();
        let dir_entries = match std::fs::read_dir(&sessions_dir) {
            Ok(d) => d,
            Err(e) => {
                let fmt = OutputFormatter::new();
                eprintln!("{}", fmt.error(&format!("Cannot read sessions directory: {}", e)));
                return Ok(DispatchOutcome::Handled);
            }
        };

        for entry in dir_entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let path_str = path.to_string_lossy().to_string();
            if let Ok(storage) = block_on(JsonlSessionStorage::open(path_str)) {
                let meta = block_on(storage.get_metadata());
                // Derive model and count from session entries
                let entries = block_on(storage.get_entries());
                let mut msg_count = 0;
                let mut model = String::new();
                for e in entries.iter().rev() {
                    if let SessionTreeEntry::Message(m) = e {
                        msg_count += 1;
                        if model.is_empty() {
                            if let AgentMessage::Assistant(a) = &m.message {
                                model = a.model.clone();
                            }
                        }
                    }
                }
                summaries.push(SessionSummary {
                    id: meta.id,
                    model,
                    msg_count,
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
        let model = Model {
            id: "mock",
            api: "mock",
        };
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
        let model = Model {
            id: "mock",
            api: "mock",
        };
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
        let mut reader = MockLineReader::new(vec!["first".into(), "second".into()]);
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
