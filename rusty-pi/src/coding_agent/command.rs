//! Frontend-neutral asynchronous slash commands.
//!
//! Commands own business behavior and return structured outcomes.  They never
//! read or write a terminal; frontends provide a [`CommandInteraction`] and
//! render the returned [`CommandResult`].

use crate::agent::session::types::SessionTreeEntry;
use crate::ai::types::{AgentMessage, AssistantContent, MessageContent};
use crate::coding_agent::prompt_session::PromptSession;
use crate::format::{OutputFormatter, SessionInfo, SessionSummary};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ── Invocation and routing ─────────────────────────────────────────────────

/// A parsed slash-command invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    /// Command name without the leading slash.
    pub name: String,
    /// Original argument text after the command name.  Only the separator
    /// whitespace is removed; whitespace inside and between arguments remains.
    pub raw_args: String,
}

impl CommandInvocation {
    pub fn new(name: impl Into<String>, raw_args: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            raw_args: raw_args.into(),
        }
    }

    /// Parse a slash input. Leading whitespace is ignored, but the argument
    /// text is otherwise retained verbatim.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim_start();
        let rest = input.strip_prefix('/')?;
        let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if name_end == 0 {
            return None;
        }
        Some(Self {
            name: rest[..name_end].to_string(),
            raw_args: rest[name_end..].trim_start().to_string(),
        })
    }

    pub fn has_args(&self) -> bool {
        !self.raw_args.trim().is_empty()
    }

    pub fn trimmed_args(&self) -> &str {
        self.raw_args.trim()
    }
}

/// The result of the registry's recognition-only phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandMatch {
    NotSlash,
    Known(CommandInvocation),
    UnknownSlash(CommandInvocation),
}

/// Shared input routing used by both REPL and TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputRoute {
    Command(CommandInvocation),
    AgentPrompt { original: String, expanded: String },
    UnknownSlash { name: String },
}

/// Resolve input without executing a command or contacting the provider.
pub fn resolve_input(input: &str, registry: &CommandRegistry, session: &PromptSession) -> InputRoute {
    match registry.resolve(input) {
        CommandMatch::Known(invocation) => InputRoute::Command(invocation),
        CommandMatch::NotSlash => InputRoute::AgentPrompt {
            original: input.to_string(),
            expanded: input.to_string(),
        },
        CommandMatch::UnknownSlash(invocation) => match session.try_expand_prompt_command(input) {
            crate::coding_agent::prompt_session::PromptExpansion::Expanded(expanded) => InputRoute::AgentPrompt {
                original: input.to_string(),
                expanded,
            },
            crate::coding_agent::prompt_session::PromptExpansion::NotMatched => {
                InputRoute::UnknownSlash { name: invocation.name }
            }
        },
    }
}

// ── Interaction port ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectRequest {
    pub prompt: String,
    pub options: Vec<String>,
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRequest {
    pub prompt: String,
    pub default: Option<String>,
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionResult<T> {
    Value(T),
    Cancelled,
    Unavailable { message: String },
}

/// Small asynchronous port for user interaction. Concrete terminal adapters
/// live outside the command domain.
#[async_trait]
pub trait CommandInteraction: Send + Sync {
    fn is_available(&self) -> bool {
        true
    }

    async fn select(&self, request: SelectRequest) -> Result<InteractionResult<String>>;
    async fn input(&self, request: InputRequest) -> Result<InteractionResult<String>>;
}

/// Interaction implementation used by the TUI until native pickers exist.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableCommandInteraction;

#[async_trait]
impl CommandInteraction for UnavailableCommandInteraction {
    fn is_available(&self) -> bool {
        false
    }

    async fn select(&self, request: SelectRequest) -> Result<InteractionResult<String>> {
        Ok(InteractionResult::Unavailable {
            message: request
                .help
                .unwrap_or_else(|| "Interactive selection is not available yet.".into()),
        })
    }

    async fn input(&self, request: InputRequest) -> Result<InteractionResult<String>> {
        Ok(InteractionResult::Unavailable {
            message: request
                .help
                .unwrap_or_else(|| "Interactive input is not available yet.".into()),
        })
    }
}

/// A deterministic interaction double useful in async command tests and
/// headless callers.
#[derive(Debug, Default)]
pub struct MockCommandInteraction {
    pub select_results: std::sync::Mutex<Vec<InteractionResult<String>>>,
    pub input_results: std::sync::Mutex<Vec<InteractionResult<String>>>,
    pub select_requests: std::sync::Mutex<Vec<SelectRequest>>,
    pub input_requests: std::sync::Mutex<Vec<InputRequest>>,
}

impl MockCommandInteraction {
    pub fn new(select_results: Vec<InteractionResult<String>>, input_results: Vec<InteractionResult<String>>) -> Self {
        Self {
            select_results: std::sync::Mutex::new(select_results),
            input_results: std::sync::Mutex::new(input_results),
            select_requests: std::sync::Mutex::new(Vec::new()),
            input_requests: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl CommandInteraction for MockCommandInteraction {
    async fn select(&self, request: SelectRequest) -> Result<InteractionResult<String>> {
        self.select_requests.lock().unwrap().push(request);
        self.select_results
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow!("MockCommandInteraction: no select result"))
    }

    async fn input(&self, request: InputRequest) -> Result<InteractionResult<String>> {
        self.input_requests.lock().unwrap().push(request);
        self.input_results
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow!("MockCommandInteraction: no input result"))
    }
}

/// A delayed interaction double for lifecycle/cancellation tests.
pub struct DelayedCommandInteraction {
    pub delay: Duration,
    pub select_result: InteractionResult<String>,
    pub input_result: InteractionResult<String>,
}

#[async_trait]
impl CommandInteraction for DelayedCommandInteraction {
    async fn select(&self, _request: SelectRequest) -> Result<InteractionResult<String>> {
        tokio::time::sleep(self.delay).await;
        Ok(self.select_result.clone())
    }

    async fn input(&self, _request: InputRequest) -> Result<InteractionResult<String>> {
        tokio::time::sleep(self.delay).await;
        Ok(self.input_result.clone())
    }
}

/// An interaction double that always reports infrastructure failure.
pub struct FailingCommandInteraction {
    pub message: String,
}

impl FailingCommandInteraction {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
impl CommandInteraction for FailingCommandInteraction {
    async fn select(&self, _request: SelectRequest) -> Result<InteractionResult<String>> {
        Err(anyhow!(self.message.clone()))
    }

    async fn input(&self, _request: InputRequest) -> Result<InteractionResult<String>> {
        Err(anyhow!(self.message.clone()))
    }
}

// ── Command contract ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandResult {
    Message(String),
    Error(String),
    Help(Vec<CommandHelpItem>),
    ModelChanged {
        model: String,
    },
    Sessions {
        sessions: Vec<SessionSummary>,
        skipped: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandHelpItem {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandControl {
    Continue,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    pub result: Option<CommandResult>,
    pub control: CommandControl,
}

impl CommandOutcome {
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            result: Some(CommandResult::Message(message.into())),
            control: CommandControl::Continue,
        }
    }

    pub fn none() -> Self {
        Self {
            result: None,
            control: CommandControl::Continue,
        }
    }

    pub fn quit() -> Self {
        Self {
            result: None,
            control: CommandControl::Quit,
        }
    }
}

pub struct CommandContext<'a> {
    pub session: &'a mut PromptSession,
    pub interaction: &'a dyn CommandInteraction,
    pub cancellation: CancellationToken,
}

impl<'a> CommandContext<'a> {
    pub fn new(
        session: &'a mut PromptSession,
        interaction: &'a dyn CommandInteraction,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            session,
            interaction,
            cancellation,
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

#[async_trait]
pub trait Command: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    async fn execute(&self, invocation: &CommandInvocation, context: &mut CommandContext<'_>)
    -> Result<CommandOutcome>;
}

pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
    help_enabled: bool,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            help_enabled: false,
        }
    }

    pub fn register(&mut self, command: Box<dyn Command>) {
        if let Some(index) = self.commands.iter().position(|item| item.name() == command.name()) {
            self.commands[index] = command;
        } else {
            self.commands.push(command);
        }
    }

    pub fn register_help(&mut self) {
        self.help_enabled = true;
    }

    /// Recognition only. Unknown slash input is returned for the prompt
    /// resolver instead of being turned into a user-facing error here.
    pub fn resolve(&self, input: &str) -> CommandMatch {
        let Some(invocation) = CommandInvocation::parse(input) else {
            return CommandMatch::NotSlash;
        };
        let known = invocation.name == "help" && self.help_enabled
            || self.commands.iter().any(|command| command.name() == invocation.name);
        if known {
            CommandMatch::Known(invocation)
        } else {
            CommandMatch::UnknownSlash(invocation)
        }
    }

    pub async fn dispatch(
        &self,
        invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        if invocation.name == "help" && self.help_enabled {
            return Ok(CommandOutcome {
                result: Some(CommandResult::Help(self.help_items())),
                control: CommandControl::Continue,
            });
        }
        let command = self
            .commands
            .iter()
            .find(|command| command.name() == invocation.name)
            .ok_or_else(|| anyhow!("command '{}' is not registered", invocation.name))?;
        command.execute(invocation, context).await
    }

    pub fn help_items(&self) -> Vec<CommandHelpItem> {
        let mut items = Vec::with_capacity(self.commands.len() + usize::from(self.help_enabled));
        if self.help_enabled {
            items.push(CommandHelpItem {
                name: "help".into(),
                description: "Show this help message".into(),
            });
        }
        items.extend(self.commands.iter().map(|command| CommandHelpItem {
            name: command.name().to_string(),
            description: command.description().to_string(),
        }));
        items
    }

    pub fn help_text(&self) -> String {
        format_help(&self.help_items())
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Frontend-neutral built-in command composition.
pub fn builtin_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    registry.register_help();
    registry.register(Box::new(ExitCommand));
    registry.register(Box::new(QuitCommand));
    registry.register(Box::new(ModelCommand));
    registry.register(Box::new(ContextCommand));
    registry.register(Box::new(SessionCommand));
    registry.register(Box::new(TreeCommand));
    registry.register(Box::new(ListSessionsCommand));
    registry
}

// ── Built-ins ───────────────────────────────────────────────────────────────

pub struct ExitCommand;

#[async_trait]
impl Command for ExitCommand {
    fn name(&self) -> &str {
        "exit"
    }
    fn description(&self) -> &str {
        "Exit the REPL"
    }
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        Ok(CommandOutcome::quit())
    }
}

pub struct QuitCommand;

#[async_trait]
impl Command for QuitCommand {
    fn name(&self) -> &str {
        "quit"
    }
    fn description(&self) -> &str {
        "Exit the REPL"
    }
    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        _context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        Ok(CommandOutcome::quit())
    }
}

pub struct ModelCommand;

#[async_trait]
impl Command for ModelCommand {
    fn name(&self) -> &str {
        "model"
    }
    fn description(&self) -> &str {
        "Switch model"
    }

    async fn execute(
        &self,
        invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        let models: Vec<_> = context.session.agent().list_models().into_iter().cloned().collect();
        let ids: Vec<String> = models.iter().map(|model| model.id.to_string()).collect();
        let current = context.session.model().id.to_string();

        let selected = if invocation.has_args() {
            let requested = invocation.trimmed_args();
            if !ids.iter().any(|id| id == requested) {
                return Ok(CommandOutcome {
                    result: Some(CommandResult::Error(format!(
                        "Model '{}' not found. Available models: {}",
                        requested,
                        ids.join(", ")
                    ))),
                    control: CommandControl::Continue,
                });
            }
            requested.to_string()
        } else if models.is_empty() && !context.interaction.is_available() {
            return Ok(CommandOutcome::message(
                "TUI model picker is not available yet.\nUse: /model <model-id>\n\nAvailable models:\n",
            ));
        } else if models.is_empty() {
            return Ok(CommandOutcome::message(format!(
                "Current model: {current}. This provider doesn't support runtime model switching."
            )));
        } else {
            if context.cancelled() {
                return Ok(CommandOutcome::none());
            }
            match context
                .interaction
                .select(SelectRequest {
                    prompt: "Select model:".into(),
                    options: ids.clone(),
                    help: Some(format!(
                        "TUI model picker is not available yet.\nUse: /model <model-id>\n\nAvailable models:\n{}",
                        ids.iter().map(|id| format!("  {id}")).collect::<Vec<_>>().join("\n")
                    )),
                })
                .await?
            {
                InteractionResult::Value(value) => value,
                InteractionResult::Cancelled => return Ok(CommandOutcome::none()),
                InteractionResult::Unavailable { message } => {
                    return Ok(CommandOutcome {
                        result: Some(CommandResult::Message(format!(
                            "{message}\n\nAvailable models:\n{}",
                            ids.iter().map(|id| format!("  {id}")).collect::<Vec<_>>().join("\n")
                        ))),
                        control: CommandControl::Continue,
                    });
                }
            }
        };

        if selected == current {
            return Ok(CommandOutcome::message(format!("Already using {selected}")));
        }
        let model = models
            .into_iter()
            .find(|model| model.id == selected)
            .ok_or_else(|| anyhow!("selected model disappeared from provider model list"))?;
        context.session.switch_model(model);
        Ok(CommandOutcome {
            result: Some(CommandResult::ModelChanged { model: selected }),
            control: CommandControl::Continue,
        })
    }
}

pub struct ContextCommand;

#[async_trait]
impl Command for ContextCommand {
    fn name(&self) -> &str {
        "context"
    }
    fn description(&self) -> &str {
        "Inject a file into the system prompt"
    }

    async fn execute(
        &self,
        invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        let path_text = if invocation.has_args() {
            invocation.trimmed_args().to_string()
        } else {
            if context.cancelled() {
                return Ok(CommandOutcome::none());
            }
            match context
                .interaction
                .input(InputRequest {
                    prompt: "File path:".into(),
                    default: None,
                    help: Some("TUI file picker is not available yet.\nUse: /context <path>".into()),
                })
                .await?
            {
                InteractionResult::Value(value) => value,
                InteractionResult::Cancelled => return Ok(CommandOutcome::none()),
                InteractionResult::Unavailable { message } => {
                    return Ok(CommandOutcome::message(message));
                }
            }
        };

        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let path = PathBuf::from(&path_text);
        let path = if path.is_absolute() {
            path
        } else {
            context.session.cwd().join(path)
        };
        let content = {
            let read = tokio::fs::read_to_string(path.clone());
            tokio::pin!(read);
            tokio::select! {
                _ = context.cancellation.cancelled() => return Ok(CommandOutcome::none()),
                result = &mut read => result.map_err(|error| anyhow!("Cannot read {}: {}", path_text, error))?,
            }
        };
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let size_kb = content.len() / 1024;
        context.session.add_context_file(path, content);
        Ok(CommandOutcome::message(format!(
            "Added {} ({}KB) to system prompt",
            path_text, size_kb
        )))
    }
}

pub struct SessionCommand;

#[async_trait]
impl Command for SessionCommand {
    fn name(&self) -> &str {
        "session"
    }
    fn description(&self) -> &str {
        "Show current session information"
    }

    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let session = context.session.session();
        let metadata = session.get_metadata().await;
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let (total, _, _, _) = session.count_messages().await;
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let model = session.derive_model().await.unwrap_or_default();
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let info = SessionInfo {
            id: metadata.id,
            model,
            msg_count: total,
            cwd: metadata.cwd,
        };
        Ok(CommandOutcome::message(OutputFormatter::new().session_info(&info)))
    }
}

pub struct TreeCommand;

#[async_trait]
impl Command for TreeCommand {
    fn name(&self) -> &str {
        "tree"
    }
    fn description(&self) -> &str {
        "Show session tree structure"
    }

    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let entries = context.session.session().get_entries().await;
        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        if entries.is_empty() {
            return Ok(CommandOutcome::message("(empty session)"));
        }

        let all_ids: HashSet<&str> = entries.iter().map(|entry| entry.id()).collect();
        let mut children: HashMap<Option<String>, Vec<&SessionTreeEntry>> = HashMap::new();
        for entry in &entries {
            children
                .entry(entry.parent_id().map(str::to_owned))
                .or_default()
                .push(entry);
        }
        let roots: Vec<&SessionTreeEntry> = entries
            .iter()
            .filter(|entry| entry.parent_id().is_none_or(|parent| !all_ids.contains(parent)))
            .collect();

        fn label(entry: &SessionTreeEntry) -> String {
            match entry {
                SessionTreeEntry::Message(message) => match &message.message {
                    AgentMessage::User(user) => format!("user: {}", preview_message(&user.content)),
                    AgentMessage::Assistant(assistant) => {
                        let text = assistant
                            .content
                            .first()
                            .map(|content| match content {
                                AssistantContent::Text { text } => truncate_preview(text, 60),
                                _ => "(tool call)".into(),
                            })
                            .unwrap_or_default();
                        format!("assistant: {text}")
                    }
                    AgentMessage::ToolResult(tool) => format!("tool: {}", tool.tool_name),
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
            if let Some(kids) = children.get(&Some(parent_id.to_string())) {
                for (index, child) in kids.iter().enumerate() {
                    let last = index + 1 == kids.len();
                    let connector = if last { "└── " } else { "├── " };
                    let continuation = if last { "    " } else { "│   " };
                    out.push_str(&format!("{prefix}{connector}{}\n", label(child)));
                    render_children(out, child.id(), children, &format!("{prefix}{continuation}"));
                }
            }
        }

        let mut output = String::new();
        for (index, root) in roots.iter().enumerate() {
            let last = index + 1 == roots.len();
            let connector = if last { "└── " } else { "├── " };
            let prefix = if last { "    " } else { "│   " };
            output.push_str(&format!("{connector}{}\n", label(root)));
            render_children(&mut output, root.id(), &children, prefix);
        }
        Ok(CommandOutcome::message(output))
    }
}

fn preview_message(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(text) => truncate_preview(text, 60),
        _ => "(non-text)".into(),
    }
}

/// Character-count truncation used by tree previews. It never slices UTF-8.
pub fn truncate_preview(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return text.chars().take(max_chars).collect();
    }
    format!("{}...", text.chars().take(max_chars - 3).collect::<String>())
}

pub struct ListSessionsCommand;

#[async_trait]
impl Command for ListSessionsCommand {
    fn name(&self) -> &str {
        "list-sessions"
    }
    fn description(&self) -> &str {
        "List all saved sessions"
    }

    async fn execute(
        &self,
        _invocation: &CommandInvocation,
        context: &mut CommandContext<'_>,
    ) -> Result<CommandOutcome> {
        use crate::agent::session::jsonl::JsonlSessionStorage;
        use crate::agent::session::storage::SessionStorage;

        if context.cancelled() {
            return Ok(CommandOutcome::none());
        }
        let sessions_dir = context.session.agent_dir().join("sessions");
        let directory = match tokio::fs::read_dir(&sessions_dir).await {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CommandOutcome::message(format!(
                    "No sessions directory found at: {}",
                    sessions_dir.display()
                )));
            }
            Err(error) => {
                return Ok(CommandOutcome {
                    result: Some(CommandResult::Error(format!("Cannot read sessions directory: {error}"))),
                    control: CommandControl::Continue,
                });
            }
        };

        let mut directory = directory;
        let mut summaries = Vec::new();
        let mut skipped = 0;
        loop {
            let entry = tokio::select! {
                _ = context.cancellation.cancelled() => return Ok(CommandOutcome::none()),
                result = directory.next_entry() => result?,
            };
            let Some(entry) = entry else { break };
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            if context.cancelled() {
                return Ok(CommandOutcome::none());
            }
            let path_text = path.to_string_lossy().to_string();
            let storage = tokio::select! {
                _ = context.cancellation.cancelled() => return Ok(CommandOutcome::none()),
                result = JsonlSessionStorage::open(path_text) => result,
            };
            let Ok(storage) = storage else {
                skipped += 1;
                continue;
            };
            if context.cancelled() {
                return Ok(CommandOutcome::none());
            }
            let metadata = storage.get_metadata().await;
            let entries = storage.get_entries().await;
            if context.cancelled() {
                return Ok(CommandOutcome::none());
            }
            let mut message_count = 0;
            let mut model = String::new();
            for entry in entries.iter().rev() {
                if let SessionTreeEntry::Message(message) = entry {
                    message_count += 1;
                    if model.is_empty()
                        && let AgentMessage::Assistant(assistant) = &message.message
                    {
                        model = assistant.model.clone();
                    }
                }
            }
            summaries.push(SessionSummary {
                id: metadata.id,
                model,
                msg_count: message_count,
                created: metadata.created_at,
            });
        }
        summaries.sort_by(|left, right| right.created.cmp(&left.created));
        Ok(CommandOutcome {
            result: Some(CommandResult::Sessions {
                sessions: summaries,
                skipped,
            }),
            control: CommandControl::Continue,
        })
    }
}

fn format_help(items: &[CommandHelpItem]) -> String {
    let mut output = String::from("\n  Commands:\n");
    for item in items {
        use std::fmt::Write;
        let _ = writeln!(output, "    /{:<12} {}", item.name, item.description);
    }
    output.push_str("\n  Tips:\n");
    output.push_str("    - Up/down arrows navigate command history\n");
    output.push_str("    - Ctrl+C at prompt exits\n");
    output.push_str("    - Ctrl+C during agent run aborts the current round\n");
    output.push_str("    - Type any text to chat with the agent\n");
    output
}

// ── Line reader boundary ────────────────────────────────────────────────────

pub trait LineReader {
    fn readline(&mut self, prompt: &str) -> Result<String, rustyline::error::ReadlineError>;
    fn add_history_entry(&mut self, line: &str);
    fn save_history(&mut self, _path: &std::path::Path) -> std::result::Result<(), rustyline::error::ReadlineError> {
        Ok(())
    }
}

pub struct MockLineReader {
    pub lines: Vec<String>,
    pub history: Vec<String>,
    index: usize,
}

impl MockLineReader {
    pub fn new(lines: Vec<String>) -> Self {
        Self {
            lines,
            history: Vec::new(),
            index: 0,
        }
    }
}

impl LineReader for MockLineReader {
    fn readline(&mut self, _prompt: &str) -> Result<String, rustyline::error::ReadlineError> {
        if let Some(line) = self.lines.get(self.index).cloned() {
            self.index += 1;
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
    use std::path::Path;

    fn session() -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::text("reply")),
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

    fn session_with_resources(template_paths: Vec<PathBuf>, skill_paths: Vec<PathBuf>) -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::text("reply")),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/.pi/agent"),
            template_paths,
            skill_paths,
            false,
            None,
            vec![],
        )
    }

    fn session_with_models() -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::with_models(
                vec![crate::ai::mock::MockStep::Text("reply".into())],
                vec![
                    Model {
                        id: "mock",
                        api: "mock",
                    },
                    Model {
                        id: "codex",
                        api: "mock",
                    },
                ],
            )),
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

    fn command_context<'a>(
        session: &'a mut PromptSession,
        interaction: &'a dyn CommandInteraction,
    ) -> CommandContext<'a> {
        CommandContext::new(session, interaction, CancellationToken::new())
    }

    #[test]
    fn invocation_preserves_argument_whitespace() {
        let invocation = CommandInvocation::parse("/context   ./folder/file with spaces.md ").unwrap();
        assert_eq!(invocation.name, "context");
        assert_eq!(invocation.raw_args, "./folder/file with spaces.md ");
        assert_eq!(invocation.trimmed_args(), "./folder/file with spaces.md");
    }

    #[test]
    fn registry_only_recognizes_commands() {
        let registry = builtin_registry();
        assert!(matches!(registry.resolve("hello"), CommandMatch::NotSlash));
        assert!(matches!(registry.resolve("/help"), CommandMatch::Known(_)));
        assert!(matches!(registry.resolve("/unknown"), CommandMatch::UnknownSlash(_)));
    }

    #[test]
    fn shared_router_prioritizes_builtins_and_expands_resources() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("help.md"), "template help").unwrap();
        std::fs::write(directory.path().join("review.md"), "Review $1").unwrap();
        let skill_dir = directory.path().join("known-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\ndescription: known\n---\n\nDo review").unwrap();
        let session = session_with_resources(
            vec![directory.path().join("help.md"), directory.path().join("review.md")],
            vec![skill_dir],
        );
        let registry = builtin_registry();

        assert!(
            matches!(resolve_input("hello", &registry, &session), InputRoute::AgentPrompt { expanded, .. } if expanded == "hello")
        );
        assert!(matches!(
            resolve_input("/help", &registry, &session),
            InputRoute::Command(_)
        ));
        assert!(
            matches!(resolve_input("/review src/", &registry, &session), InputRoute::AgentPrompt { expanded, .. } if expanded == "Review src/")
        );
        assert!(matches!(
            resolve_input("/skill:known-skill", &registry, &session),
            InputRoute::AgentPrompt { .. }
        ));
        assert!(
            matches!(resolve_input("/not-known", &registry, &session), InputRoute::UnknownSlash { name } if name == "not-known")
        );
        assert!(
            matches!(resolve_input("/skill:not-known", &registry, &session), InputRoute::UnknownSlash { name } if name == "skill:not-known")
        );
    }

    #[test]
    fn expansion_equal_to_input_is_still_a_match() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("same.md"), "/same").unwrap();
        let session = session_with_resources(vec![directory.path().join("same.md")], vec![]);
        let expansion = session.try_expand_prompt_command("/same");
        assert_eq!(
            expansion,
            crate::coding_agent::prompt_session::PromptExpansion::Expanded("/same".into())
        );
    }

    #[tokio::test]
    async fn unavailable_model_picker_is_structured_and_non_blocking() {
        let mut session = session();
        let interaction = UnavailableCommandInteraction;
        let invocation = CommandInvocation::new("model", "");
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand.execute(&invocation, &mut context).await.unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Message(message)) if message.contains("Use: /model <model-id>"))
        );
    }

    #[tokio::test]
    async fn model_argument_matches_exactly_and_switches_without_interaction() {
        let mut session = session_with_models();
        let interaction = FailingCommandInteraction::new("interaction must not be called");
        let invocation = CommandInvocation::new("model", "codex");
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand.execute(&invocation, &mut context).await.unwrap();
        assert_eq!(
            outcome.result,
            Some(CommandResult::ModelChanged { model: "codex".into() })
        );
        assert_eq!(session.model().id, "codex");
    }

    #[tokio::test]
    async fn invalid_model_lists_available_ids() {
        let mut session = session_with_models();
        let interaction = FailingCommandInteraction::new("interaction must not be called");
        let invocation = CommandInvocation::new("model", "missing");
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand.execute(&invocation, &mut context).await.unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Error(message)) if message.contains("mock") && message.contains("codex"))
        );
    }

    #[tokio::test]
    async fn model_without_argument_uses_async_interaction_and_cancel_is_continue() {
        let mut session = session_with_models();
        let interaction = MockCommandInteraction::new(vec![InteractionResult::Value("codex".into())], vec![]);
        let invocation = CommandInvocation::new("model", "");
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand.execute(&invocation, &mut context).await.unwrap();
        assert_eq!(
            outcome.result,
            Some(CommandResult::ModelChanged { model: "codex".into() })
        );
        assert_eq!(session.model().id, "codex");

        let mut session = session_with_models();
        let interaction = MockCommandInteraction::new(vec![InteractionResult::Cancelled], vec![]);
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand.execute(&invocation, &mut context).await.unwrap();
        assert_eq!(outcome, CommandOutcome::none());
        assert_eq!(session.model().id, "mock");
    }

    #[tokio::test]
    async fn context_reads_unicode_path_with_tokio_and_updates_prompt_after_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("路径 with spaces.md");
        tokio::fs::write(&path, "important context").await.unwrap();
        let mut session = session();
        let before = session.system_prompt().to_string();
        let interaction = FailingCommandInteraction::new("interaction must not be called");
        let invocation = CommandInvocation::new("context", path.to_string_lossy());
        let mut context = command_context(&mut session, &interaction);
        let outcome = ContextCommand.execute(&invocation, &mut context).await.unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Message(message)) if message.contains("路径 with spaces.md"))
        );
        assert_ne!(session.system_prompt(), before);
        assert!(session.system_prompt().contains("important context"));
    }

    #[tokio::test]
    async fn list_sessions_skips_corrupt_jsonl_and_reports_count() {
        let directory = tempfile::tempdir().unwrap();
        let sessions = directory.path().join("sessions");
        tokio::fs::create_dir_all(&sessions).await.unwrap();
        tokio::fs::write(sessions.join("broken.jsonl"), "not json\n")
            .await
            .unwrap();
        let valid = sessions.join("valid.jsonl");
        crate::agent::session::jsonl::JsonlSessionStorage::create(
            valid.to_string_lossy().to_string(),
            crate::agent::session::jsonl::JsonlSessionCreateOptions {
                session_id: "valid".into(),
                cwd: directory.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
        let provider = MockProvider::text("reply");
        let mut session = PromptSession::new(
            Box::new(provider),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            directory.path().to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        );
        let interaction = UnavailableCommandInteraction;
        let mut context = command_context(&mut session, &interaction);
        let outcome = ListSessionsCommand
            .execute(&CommandInvocation::new("list-sessions", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Sessions { sessions, skipped }) if sessions.len() == 1 && skipped == 1)
        );
    }

    #[tokio::test]
    async fn quit_is_control_not_result() {
        let registry = builtin_registry();
        let invocation = CommandInvocation::new("quit", "");
        let interaction = UnavailableCommandInteraction;
        let mut session = session();
        let token = CancellationToken::new();
        let mut context = CommandContext::new(&mut session, &interaction, token);
        let outcome = registry.dispatch(&invocation, &mut context).await.unwrap();
        assert_eq!(outcome.result, None);
        assert_eq!(outcome.control, CommandControl::Quit);
    }

    #[tokio::test]
    async fn session_command_awaits_without_nested_runtime() {
        let registry = builtin_registry();
        let invocation = CommandInvocation::new("session", "");
        let interaction = UnavailableCommandInteraction;
        let mut session = session();
        let mut context = CommandContext::new(&mut session, &interaction, CancellationToken::new());
        let outcome = registry.dispatch(&invocation, &mut context).await.unwrap();
        assert!(matches!(outcome.result, Some(CommandResult::Message(_))));
    }

    #[tokio::test]
    async fn context_cancel_does_not_mutate_prompt_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("中文 file.md");
        tokio::fs::write(&path, "context").await.unwrap();
        let provider = MockProvider::text("reply");
        let mut session = PromptSession::new(
            Box::new(provider),
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
        );
        let before = session.system_prompt().to_string();
        let token = CancellationToken::new();
        token.cancel();
        let invocation = CommandInvocation::new("context", path.to_string_lossy());
        let interaction = UnavailableCommandInteraction;
        let mut context = CommandContext::new(&mut session, &interaction, token);
        let outcome = ContextCommand.execute(&invocation, &mut context).await.unwrap();
        assert_eq!(outcome, CommandOutcome::none());
        assert_eq!(session.system_prompt(), before);
    }

    #[test]
    fn truncate_preview_is_unicode_safe() {
        let result = truncate_preview("中文🙂文本", 4);
        assert!(result.is_char_boundary(result.len()));
        assert!(result.ends_with("..."));
    }

    #[test]
    fn line_reader_is_still_testable() {
        let mut reader = MockLineReader::new(vec!["a".into()]);
        assert_eq!(reader.readline("> ").unwrap(), "a");
        reader.add_history_entry("a");
        assert_eq!(reader.history, vec!["a"]);
        assert!(Path::new("/tmp").exists());
    }

    #[test]
    fn invocation_without_arguments_is_detected() {
        let invocation = CommandInvocation::parse("/help").unwrap();
        assert_eq!(invocation.name, "help");
        assert!(!invocation.has_args());
    }

    #[test]
    fn invocation_name_never_contains_slash() {
        let invocation = CommandInvocation::parse("/tree now").unwrap();
        assert_eq!(invocation.name, "tree");
        assert!(!invocation.name.contains('/'));
    }

    #[test]
    fn registry_non_slash_is_not_a_command() {
        assert_eq!(builtin_registry().resolve("hello"), CommandMatch::NotSlash);
    }

    #[test]
    fn registry_empty_input_is_not_a_command() {
        assert_eq!(builtin_registry().resolve(""), CommandMatch::NotSlash);
    }

    #[test]
    fn help_contains_tips_and_all_builtins() {
        let text = builtin_registry().help_text();
        assert!(text.contains("Commands:"));
        assert!(text.contains("/help"));
        assert!(text.contains("/list-sessions"));
        assert!(text.contains("Ctrl+C"));
    }

    #[tokio::test]
    async fn help_dispatch_returns_metadata() {
        let registry = builtin_registry();
        let interaction = UnavailableCommandInteraction;
        let mut session = session();
        let mut context = command_context(&mut session, &interaction);
        let outcome = registry
            .dispatch(&CommandInvocation::new("help", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Help(items)) if items.iter().any(|item| item.name == "model"))
        );
    }

    #[tokio::test]
    async fn exit_and_quit_have_no_display_result() {
        let registry = builtin_registry();
        for name in ["exit", "quit"] {
            let interaction = UnavailableCommandInteraction;
            let mut session = session();
            let mut context = command_context(&mut session, &interaction);
            let outcome = registry
                .dispatch(&CommandInvocation::new(name, "ignored"), &mut context)
                .await
                .unwrap();
            assert_eq!(outcome.result, None);
            assert_eq!(outcome.control, CommandControl::Quit);
        }
    }

    #[test]
    fn outcome_helpers_separate_result_and_control() {
        assert_eq!(CommandOutcome::message("ok").control, CommandControl::Continue);
        assert_eq!(CommandOutcome::none().result, None);
        assert_eq!(CommandOutcome::quit().result, None);
    }

    #[tokio::test]
    async fn current_model_message_is_used_when_repl_provider_has_no_models() {
        let mut session = session();
        let interaction = MockCommandInteraction::new(vec![], vec![]);
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand
            .execute(&CommandInvocation::new("model", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Message(message)) if message.contains("Current model: mock"))
        );
    }

    #[tokio::test]
    async fn selecting_current_model_reports_already_using() {
        let mut session = session_with_models();
        let interaction = MockCommandInteraction::new(vec![InteractionResult::Value("mock".into())], vec![]);
        let mut context = command_context(&mut session, &interaction);
        let outcome = ModelCommand
            .execute(&CommandInvocation::new("model", ""), &mut context)
            .await
            .unwrap();
        assert_eq!(
            outcome.result,
            Some(CommandResult::Message("Already using mock".into()))
        );
    }

    #[tokio::test]
    async fn context_missing_file_is_an_error_without_mutation() {
        let mut session = session();
        let before = session.system_prompt().to_string();
        let interaction = FailingCommandInteraction::new("must not interact");
        let mut context = command_context(&mut session, &interaction);
        assert!(
            ContextCommand
                .execute(&CommandInvocation::new("context", "/missing/context"), &mut context)
                .await
                .is_err()
        );
        assert_eq!(session.system_prompt(), before);
    }

    #[tokio::test]
    async fn unavailable_context_input_returns_usage_message() {
        let mut session = session();
        let interaction = UnavailableCommandInteraction;
        let mut context = command_context(&mut session, &interaction);
        let outcome = ContextCommand
            .execute(&CommandInvocation::new("context", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Message(message)) if message.contains("Use: /context <path>"))
        );
    }

    #[tokio::test]
    async fn empty_tree_is_reported_without_provider() {
        let mut session = session();
        let interaction = UnavailableCommandInteraction;
        let mut context = command_context(&mut session, &interaction);
        let outcome = TreeCommand
            .execute(&CommandInvocation::new("tree", ""), &mut context)
            .await
            .unwrap();
        assert_eq!(outcome.result, Some(CommandResult::Message("(empty session)".into())));
    }

    #[tokio::test]
    async fn list_sessions_missing_directory_is_a_message() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = PromptSession::new(
            Box::new(MockProvider::text("reply")),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            directory.path().to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        );
        let interaction = UnavailableCommandInteraction;
        let mut context = command_context(&mut session, &interaction);
        let outcome = ListSessionsCommand
            .execute(&CommandInvocation::new("list-sessions", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Message(message)) if message.contains("No sessions directory"))
        );
    }

    #[tokio::test]
    async fn cancelled_session_and_tree_commands_return_no_result() {
        let interaction = UnavailableCommandInteraction;
        for name in ["session", "tree"] {
            let mut session = session();
            let token = CancellationToken::new();
            token.cancel();
            let mut context = CommandContext::new(&mut session, &interaction, token);
            let outcome = match name {
                "session" => SessionCommand
                    .execute(&CommandInvocation::new(name, ""), &mut context)
                    .await
                    .unwrap(),
                _ => TreeCommand
                    .execute(&CommandInvocation::new(name, ""), &mut context)
                    .await
                    .unwrap(),
            };
            assert_eq!(outcome, CommandOutcome::none());
        }
    }

    #[tokio::test]
    async fn mock_interaction_records_requests() {
        let interaction = MockCommandInteraction::new(
            vec![InteractionResult::Value("selected".into())],
            vec![InteractionResult::Value("path".into())],
        );
        let selected = interaction
            .select(SelectRequest {
                prompt: "pick".into(),
                options: vec!["selected".into()],
                help: None,
            })
            .await
            .unwrap();
        let input = interaction
            .input(InputRequest {
                prompt: "path".into(),
                default: None,
                help: None,
            })
            .await
            .unwrap();
        assert_eq!(selected, InteractionResult::Value("selected".into()));
        assert_eq!(input, InteractionResult::Value("path".into()));
        assert_eq!(interaction.select_requests.lock().unwrap().len(), 1);
        assert_eq!(interaction.input_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delayed_interaction_is_async() {
        let interaction = DelayedCommandInteraction {
            delay: Duration::from_millis(1),
            select_result: InteractionResult::Cancelled,
            input_result: InteractionResult::Cancelled,
        };
        let result = interaction
            .input(InputRequest {
                prompt: "path".into(),
                default: None,
                help: None,
            })
            .await
            .unwrap();
        assert_eq!(result, InteractionResult::Cancelled);
    }

    #[tokio::test]
    async fn failing_interaction_propagates_infrastructure_error() {
        let interaction = FailingCommandInteraction::new("terminal failed");
        let error = interaction
            .select(SelectRequest {
                prompt: "pick".into(),
                options: vec![],
                help: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("terminal failed"));
    }

    #[tokio::test]
    async fn exhausted_mock_interaction_is_an_error() {
        let interaction = MockCommandInteraction::new(vec![], vec![]);
        assert!(
            interaction
                .select(SelectRequest {
                    prompt: "pick".into(),
                    options: vec![],
                    help: None
                })
                .await
                .is_err()
        );
    }

    #[test]
    fn command_help_items_are_value_equal() {
        let left = CommandHelpItem {
            name: "x".into(),
            description: "X".into(),
        };
        let right = CommandHelpItem {
            name: "x".into(),
            description: "X".into(),
        };
        assert_eq!(left, right);
    }

    #[test]
    fn command_result_variants_are_distinct() {
        assert_ne!(CommandResult::Message("a".into()), CommandResult::Error("a".into()));
        assert_ne!(
            CommandResult::ModelChanged { model: "a".into() },
            CommandResult::Message("a".into())
        );
    }

    #[tokio::test]
    async fn session_and_tree_commands_return_structured_messages() {
        let interaction = UnavailableCommandInteraction;
        for command in ["session", "tree"] {
            let mut session = session();
            let mut context = command_context(&mut session, &interaction);
            let outcome = match command {
                "session" => SessionCommand
                    .execute(&CommandInvocation::new(command, ""), &mut context)
                    .await
                    .unwrap(),
                _ => TreeCommand
                    .execute(&CommandInvocation::new(command, ""), &mut context)
                    .await
                    .unwrap(),
            };
            assert!(matches!(outcome.result, Some(CommandResult::Message(_))));
        }
    }

    #[tokio::test]
    async fn list_sessions_empty_directory_returns_zero_skipped() {
        let directory = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(directory.path().join("sessions"))
            .await
            .unwrap();
        let mut session = PromptSession::new(
            Box::new(MockProvider::text("reply")),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            directory.path().to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        );
        let interaction = UnavailableCommandInteraction;
        let mut context = command_context(&mut session, &interaction);
        let outcome = ListSessionsCommand
            .execute(&CommandInvocation::new("list-sessions", ""), &mut context)
            .await
            .unwrap();
        assert!(
            matches!(outcome.result, Some(CommandResult::Sessions { sessions, skipped }) if sessions.is_empty() && skipped == 0)
        );
    }

    #[tokio::test]
    async fn pre_cancelled_list_sessions_leaves_no_task_or_result() {
        let directory = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(directory.path().join("sessions"))
            .await
            .unwrap();
        let mut session = PromptSession::new(
            Box::new(MockProvider::text("reply")),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            directory.path().to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        );
        let token = CancellationToken::new();
        token.cancel();
        let interaction = UnavailableCommandInteraction;
        let mut context = CommandContext::new(&mut session, &interaction, token);
        let outcome = ListSessionsCommand
            .execute(&CommandInvocation::new("list-sessions", ""), &mut context)
            .await
            .unwrap();
        assert_eq!(outcome, CommandOutcome::none());
    }

    #[tokio::test]
    async fn builtins_do_not_append_messages_or_call_provider() {
        let directory = tempfile::tempdir().unwrap();
        let context_path = directory.path().join("context.md");
        tokio::fs::write(&context_path, "context").await.unwrap();
        let provider = MockProvider::with_models(
            vec![crate::ai::mock::MockStep::Text("must not run".into())],
            vec![
                Model {
                    id: "mock",
                    api: "mock",
                },
                Model {
                    id: "codex",
                    api: "mock",
                },
            ],
        );
        let captured = provider.captured_requests_arc();
        let mut session = PromptSession::new(
            Box::new(provider),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            directory.path().to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        );
        let registry = builtin_registry();
        let interaction = UnavailableCommandInteraction;
        for invocation in [
            CommandInvocation::new("help", ""),
            CommandInvocation::new("model", "codex"),
            CommandInvocation::new("context", context_path.to_string_lossy()),
            CommandInvocation::new("session", ""),
            CommandInvocation::new("tree", ""),
            CommandInvocation::new("list-sessions", ""),
        ] {
            let mut context = CommandContext::new(&mut session, &interaction, CancellationToken::new());
            let _ = registry.dispatch(&invocation, &mut context).await.unwrap();
        }
        assert_eq!(session.session().count_messages().await.0, 0);
        assert!(captured.lock().unwrap().is_empty());
    }
}
