//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::agent::engine::AbortFlag;
use crate::ai::types::{AgentMessage, StopReason};
use crate::coding_agent::command::{
    CommandRegistry, CommandResult, ContextCommand, DispatchOutcome, ExitCommand, HelpCommand, LineReader,
    ListSessionsCommand, ModelCommand, QuitCommand, SessionCommand, TreeCommand,
};
use crate::coding_agent::prompt_session::PromptSession;
use crate::format::OutputFormatter;
use anyhow::Result;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Run configuration for the CLI.
pub struct RunConfig {
    /// Optional single-shot prompt. If `None`, enters REPL mode.
    pub prompt: Option<String>,
    /// Session that wraps the agent with prompt template and skill expansion.
    pub session: PromptSession,
}

/// Wraps `rustyline::DefaultEditor` as a [`LineReader`].
pub struct RustylineReader {
    pub inner: DefaultEditor,
}

impl RustylineReader {
    pub fn new(inner: DefaultEditor) -> Self {
        Self { inner }
    }
}

impl LineReader for RustylineReader {
    fn readline(&mut self, prompt: &str) -> Result<String, ReadlineError> {
        self.inner.readline(prompt)
    }

    fn add_history_entry(&mut self, line: &str) {
        let _ = self.inner.add_history_entry(line);
    }

    fn save_history(&mut self, path: &std::path::Path) -> std::result::Result<(), ReadlineError> {
        self.inner.save_history(path)
    }
}

/// Build the default command registry with built-in and interactive commands.
pub fn default_registry() -> CommandRegistry {
    use crate::coding_agent::picker::RealPicker;

    let mut registry = CommandRegistry::new();
    registry.register(Box::new(HelpCommand));
    registry.register(Box::new(ExitCommand));
    registry.register(Box::new(QuitCommand));
    registry.register(Box::new(ModelCommand::new(Box::new(RealPicker))));
    registry.register(Box::new(ContextCommand::new(Box::new(RealPicker))));
    registry.register(Box::new(SessionCommand));
    registry.register(Box::new(TreeCommand));
    registry.register(Box::new(ListSessionsCommand));
    registry
}

/// Build a startup banner string using the OutputFormatter.
pub fn startup_banner(provider: &str, model: &str, session_id: &str) -> String {
    let fmt = OutputFormatter::new();
    fmt.banner(provider, model, session_id)
}

/// Render a CommandResult to the terminal (stdout/stderr).
pub fn render_command_result(result: &CommandResult) {
    match result {
        CommandResult::Message(text) => {
            println!("{}", text);
        }
        CommandResult::Error(msg) => {
            let fmt = OutputFormatter::new();
            eprintln!("{}", fmt.error(msg));
        }
        CommandResult::Help(items) => {
            println!("\n  Commands:");
            for item in items {
                println!("    /{:<12} {}", item.name, item.description);
            }
            println!("\n  Tips:");
            println!("    - Up/down arrows navigate command history");
            println!("    - Ctrl+C at prompt exits");
            println!("    - Ctrl+C during agent run aborts the current round");
            println!("    - Type any text to chat with the agent");
        }
        CommandResult::ModelChanged { model } => {
            println!("Switched to {}", model);
        }
        CommandResult::Sessions(sessions) => {
            if sessions.is_empty() {
                println!("No sessions found.");
            } else {
                println!("Available sessions:");
                for s in sessions {
                    println!(
                        "  {} | model: {} | msgs: {} | created: {}",
                        s.id, s.model, s.msg_count, s.created
                    );
                }
            }
        }
        CommandResult::Quit => {
            // Handled by the caller via DispatchOutcome::Exit
        }
        CommandResult::Noop => {}
    }
}

/// Run the CLI with the given configuration.
pub async fn run(config: RunConfig) -> Result<()> {
    let mut session = config.session;

    match config.prompt {
        Some(prompt) => run_single_shot(&mut session, &prompt).await,
        None => run_repl(&mut session).await,
    }
}

/// Helper: run an agent with Ctrl+C abort support and tool event formatting.
/// Returns `true` if the run was aborted, `false` otherwise.
async fn run_with_abort(session: &mut PromptSession, prompt: &str) -> bool {
    // Expand templates/skills before borrowing agent
    let expanded = session.expand(prompt);

    let agent = session.agent();
    let abort_token: AbortFlag = CancellationToken::new();
    agent.set_abort_flag(abort_token.clone());

    let formatter = Arc::new(OutputFormatter::new());

    let buf = Arc::new(Mutex::new(String::new()));
    let buf_cb = buf.clone();
    agent.on_text(move |delta| {
        print!("{}", delta);
        let _ = io::stdout().flush();
        buf_cb.lock().unwrap().push_str(delta);
    });

    // Register tool event callbacks
    {
        let fmt = formatter.clone();
        agent.on_tool_start(move |name, args| {
            print!("{}", fmt.tool_start(name, args));
            let _ = io::stdout().flush();
        });
    }
    {
        let fmt = formatter.clone();
        agent.on_tool_end(move |name, duration| {
            print!("{}", fmt.tool_end(name, duration));
            let _ = io::stdout().flush();
        });
    }

    let run_future = agent.run(&expanded);
    tokio::pin!(run_future);

    let was_aborted = tokio::select! {
        result = &mut run_future => {
            if let Err(e) = result {
                eprintln!("\n[error] {}", e);
            }
            false
        }
        _ = tokio::signal::ctrl_c() => {
            let fmt = OutputFormatter::new();
            eprintln!("{}", fmt.interrupt());
            abort_token.cancel();
            true
        }
    };

    // Print trailing newline if needed
    {
        let output = buf.lock().unwrap();
        if !output.ends_with('\n') && !output.is_empty() {
            println!();
        }
    }

    was_aborted
}

/// Run a single prompt and print the response, then exit.
async fn run_single_shot(session: &mut PromptSession, prompt: &str) -> Result<()> {
    run_with_abort(session, prompt).await;
    Ok(())
}

/// Resolve history path given a home directory string.
fn history_path_for_home(home: &str) -> PathBuf {
    PathBuf::from(home).join(".pi").join("agent").join("repl-history.txt")
}

/// Get the REPL history file path.
fn history_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    history_path_for_home(&home)
}

/// Enter the interactive REPL loop with default rustyline reader and commands.
async fn run_repl(session: &mut PromptSession) -> Result<()> {
    let hist_path = history_path();
    if let Some(parent) = hist_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let mut rl = DefaultEditor::new().map_err(|e| anyhow::anyhow!("Failed to create REPL editor: {}", e))?;
    let _ = rl.load_history(&hist_path);
    let mut reader = RustylineReader::new(rl);

    let registry = default_registry();

    run_repl_with(session, &registry, &mut reader, &hist_path).await
}

/// Core REPL loop, parameterized over reader and registry for testability.
async fn run_repl_with(
    session: &mut PromptSession,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &PathBuf,
) -> Result<()> {
    let meta = session.session().get_metadata().await;
    let model = session.model().id;
    let banner = startup_banner("rusty-pi", model, &meta.id);
    println!("{}", banner);
    println!("Type '/help' for commands\n");

    loop {
        let line = match reader.readline("> ") {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                break;
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(e) => {
                eprintln!("[error] Input error: {}", e);
                break;
            }
        };

        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }

        reader.add_history_entry(&trimmed);

        // Try command dispatch (dispatch handles /help internally)
        match registry.dispatch(&trimmed, session)? {
            DispatchOutcome::Exit => break,
            DispatchOutcome::Handled(result) => {
                render_command_result(&result);
                continue;
            }
            DispatchOutcome::NotACommand => {
                // Treat as a prompt for the agent
            }
        }

        let aborted = run_with_abort(session, &trimmed).await;

        if !aborted {
            let agent = session.agent();
            let msgs = agent.messages().await;
            if let Some(AgentMessage::Assistant(a)) = msgs.last()
                && a.stop_reason == StopReason::Error
                && let Some(err) = &a.error_message
            {
                let fmt = OutputFormatter::new();
                eprintln!("{}", fmt.error(err));
            }
        }
        println!();
    }

    // Save history (best-effort)
    let _ = reader.save_history(history_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::command::MockLineReader;

    /// A minimal PromptSession that doesn't connect to real providers.
    fn mock_session() -> PromptSession {
        let provider = crate::ai::mock::MockProvider::text("mock reply");
        let model = crate::ai::providers::Model {
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

    #[tokio::test]
    async fn repl_with_exit_command() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();

        // Should have processed the exit without error
        assert!(reader.history.contains(&"/exit".to_string()));
    }

    #[tokio::test]
    async fn repl_with_quit_command() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["/quit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_with_help_command() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["/help".into(), "/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_with_unknown_command() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["/unknown".into(), "/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_with_regular_prompt_then_exit() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["hello agent".into(), "/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();

        assert!(reader.history.contains(&"hello agent".to_string()));
        assert!(reader.history.contains(&"/exit".to_string()));
    }

    #[tokio::test]
    async fn repl_with_empty_lines_skips() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["".into(), "  ".into(), "/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();

        // Empty lines shouldn't be added to history
        assert_eq!(reader.history.len(), 1);
        assert!(reader.history.contains(&"/exit".to_string()));
    }

    #[tokio::test]
    async fn repl_eof_triggers_exit() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec![]); // empty → immediate EOF
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn repl_multiple_prompts() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["first".into(), "second".into(), "/exit".into()]);
        let hist = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with(&mut session, &registry, &mut reader, &hist)
            .await
            .unwrap();

        assert_eq!(reader.history.len(), 3);
    }

    // ── Existing tests (unchanged) ─────────────────────────────────────

    #[test]
    fn history_path_uses_home_env() {
        let path = history_path_for_home("/home/user");
        assert_eq!(path, PathBuf::from("/home/user/.pi/agent/repl-history.txt"));
    }

    #[test]
    fn history_path_handles_trailing_slash() {
        let path = history_path_for_home("/home/user/");
        assert_eq!(path, PathBuf::from("/home/user/.pi/agent/repl-history.txt"));
    }
}
