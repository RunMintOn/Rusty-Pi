//! REPL — Read-Eval-Print Loop for interactive chat.

use crate::ai::types::StopReason;
use crate::coding_agent::command::{
    CommandRegistry, ContextCommand, DispatchOutcome, ExitCommand, HelpCommand, LineReader, ListSessionsCommand,
    ModelCommand, QuitCommand, SessionCommand, TreeCommand,
};
use crate::coding_agent::prompt_session::PromptSession;
use crate::format::OutputFormatter;
use crate::frontends::PrintFrontend;
use anyhow::Result;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::path::{Path, PathBuf};
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

/// Run the CLI with the given configuration.
pub async fn run(config: RunConfig) -> Result<()> {
    let mut session = config.session;

    match config.prompt {
        Some(prompt) => {
            let outcome = run_single_shot(&mut session, &prompt).await?;
            match outcome {
                crate::frontends::print::PrintRunOutcome::Finished(StopReason::Error) => {
                    Err(anyhow::anyhow!("Provider/model error"))
                }
                crate::frontends::print::PrintRunOutcome::Finished(StopReason::Aborted)
                | crate::frontends::print::PrintRunOutcome::Aborted => std::process::exit(130),
                crate::frontends::print::PrintRunOutcome::Finished(_) => Ok(()),
                crate::frontends::print::PrintRunOutcome::Failed(e) => Err(anyhow::anyhow!("Run failed: {}", e)),
            }
        }
        None => run_repl(&mut session).await,
    }
}

/// Helper: run an agent with Ctrl+C abort support using the Print Run Driver.
/// Returns the run outcome.
async fn run_with_abort<O: crate::frontends::print::FrontendOutput>(
    session: &mut PromptSession,
    prompt: &str,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
) -> std::io::Result<crate::frontends::print::PrintRunOutcome> {
    use crate::frontends::print::drive_print_run;

    let expanded = session.expand(prompt);
    let agent = session.agent();
    let run_token = CancellationToken::new();

    drive_print_run(agent, &expanded, frontend, run_token).await
}

/// Run a single prompt and print the response, then exit.
/// Uses PrintFrontend for event-based output with proper exit codes.
async fn run_single_shot(
    session: &mut PromptSession,
    prompt: &str,
) -> Result<crate::frontends::print::PrintRunOutcome> {
    use crate::frontends::print::{PrintFrontend, RealOutput, drive_print_run};

    let expanded = session.expand(prompt);
    let agent = session.agent();
    let run_token = CancellationToken::new();
    let mut frontend = PrintFrontend::<RealOutput>::new();

    Ok(drive_print_run(agent, &expanded, &mut frontend, run_token).await?)
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
    history_path: &Path,
) -> Result<()> {
    let mut frontend = PrintFrontend::new();
    run_repl_with_frontend(session, registry, reader, history_path, &mut frontend).await
}

/// Core REPL loop with an injectable frontend output sink for tests.
async fn run_repl_with_frontend<O: crate::frontends::print::FrontendOutput>(
    session: &mut PromptSession,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &Path,
    frontend: &mut PrintFrontend<O>,
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
                frontend.handle_command_result(&result)?;
                continue;
            }
            DispatchOutcome::NotACommand => {
                // Treat as a prompt for the agent
            }
        }

        // Agent-visible output is rendered only by drive_print_run through
        // the frontend. Do not reconstruct errors from session messages.
        let _outcome = run_with_abort(session, &trimmed, frontend).await?;
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
    async fn provider_error_in_repl_is_rendered_once_by_the_event_path() {
        let provider = crate::ai::mock::MockProvider::new(vec![crate::ai::mock::MockStep::Error("API error".into())]);
        let model = crate::ai::providers::Model {
            id: "mock",
            api: "mock",
        };
        let mut session = PromptSession::new(
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
        );
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["trigger provider error".into(), "/exit".into()]);
        let mut frontend = PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());
        let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        run_repl_with_frontend(&mut session, &registry, &mut reader, &history, &mut frontend)
            .await
            .unwrap();

        let stderr = frontend.output().stderr_str();
        assert_eq!(stderr.matches("API error").count(), 1);
        assert_eq!(stderr.matches("Provider error").count(), 1);
    }

    #[tokio::test]
    async fn repl_propagates_command_result_output_failure() {
        let mut session = mock_session();
        let registry = default_registry();
        let mut reader = MockLineReader::new(vec!["/help".into()]);
        let mut frontend = PrintFrontend::with_output(crate::frontends::print::FailingOutput);
        let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");

        let error = run_repl_with_frontend(&mut session, &registry, &mut reader, &history, &mut frontend)
            .await
            .expect_err("command output failure must not be swallowed");
        assert!(error.to_string().contains("stdout write failed"));
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
