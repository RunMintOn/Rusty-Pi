//! Thin blocking-line REPL frontend.
//!
//! The REPL owns rustyline and the `inquire` adapter, while command behavior,
//! routing, expansion, and rendering boundaries remain shared with the TUI.

use crate::ai::types::StopReason;
use crate::coding_agent::command::{
    CommandContext, CommandInteraction, CommandOutcome, CommandRegistry, InputRequest, InputRoute, InteractionResult,
    LineReader, SelectRequest, builtin_registry, resolve_input,
};
use crate::coding_agent::prompt_session::PromptSession;
use crate::format::OutputFormatter;
use anyhow::Result;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// Blocking REPL interaction adapter. Blocking prompts are isolated from the
/// Tokio worker with `spawn_blocking`; user cancellation is not an error.
pub struct InquireCommandInteraction;

#[async_trait::async_trait]
impl CommandInteraction for InquireCommandInteraction {
    async fn select(&self, request: SelectRequest) -> Result<InteractionResult<String>> {
        let result =
            tokio::task::spawn_blocking(move || inquire::Select::new(&request.prompt, request.options).prompt())
                .await
                .map_err(|error| anyhow::anyhow!("inquire task failed: {error}"))?;
        map_inquire_result(result)
    }

    async fn input(&self, request: InputRequest) -> Result<InteractionResult<String>> {
        let result = tokio::task::spawn_blocking(move || {
            let mut prompt = inquire::Text::new(&request.prompt);
            if let Some(default) = request.default.as_deref() {
                prompt = prompt.with_default(default);
            }
            if let Some(help) = request.help.as_deref() {
                prompt = prompt.with_help_message(help);
            }
            prompt.prompt()
        })
        .await
        .map_err(|error| anyhow::anyhow!("inquire task failed: {error}"))?;
        map_inquire_result(result)
    }
}

fn map_inquire_result<T>(result: std::result::Result<T, inquire::InquireError>) -> Result<InteractionResult<T>> {
    match result {
        Ok(value) => Ok(InteractionResult::Value(value)),
        Err(inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted) => {
            Ok(InteractionResult::Cancelled)
        }
        Err(error) => Err(anyhow::anyhow!(error)),
    }
}

/// Run configuration for the CLI.
pub struct RunConfig {
    pub prompt: Option<String>,
    pub session: PromptSession,
}

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

    fn save_history(&mut self, path: &Path) -> std::result::Result<(), ReadlineError> {
        self.inner.save_history(path)
    }
}

pub fn startup_banner(provider: &str, model: &str, session_id: &str) -> String {
    OutputFormatter::new().banner(provider, model, session_id)
}

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
                crate::frontends::print::PrintRunOutcome::Failed(error) => Err(anyhow::anyhow!("Run failed: {error}")),
            }
        }
        None => run_repl(&mut session).await,
    }
}

async fn run_with_abort<O: crate::frontends::print::FrontendOutput>(
    session: &mut PromptSession,
    expanded_prompt: &str,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
) -> std::io::Result<crate::frontends::print::PrintRunOutcome> {
    crate::frontends::print::drive_print_run(session.agent(), expanded_prompt, frontend, CancellationToken::new()).await
}

async fn run_single_shot(
    session: &mut PromptSession,
    prompt: &str,
) -> Result<crate::frontends::print::PrintRunOutcome> {
    use crate::frontends::print::{PrintFrontend, RealOutput, drive_print_run};
    let expanded = session.expand(prompt);
    Ok(drive_print_run(
        session.agent(),
        &expanded,
        &mut PrintFrontend::<RealOutput>::new(),
        CancellationToken::new(),
    )
    .await?)
}

fn history_path_for_home(home: &str) -> PathBuf {
    PathBuf::from(home).join(".pi").join("agent").join("repl-history.txt")
}

fn history_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    history_path_for_home(&home)
}

async fn run_repl(session: &mut PromptSession) -> Result<()> {
    let hist_path = history_path();
    if let Some(parent) = hist_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let editor = DefaultEditor::new().map_err(|error| anyhow::anyhow!("Failed to create REPL editor: {error}"))?;
    let mut reader = RustylineReader::new(editor);
    let _ = reader.inner.load_history(&hist_path);
    let registry = builtin_registry();
    run_repl_with(session, &registry, &mut reader, &hist_path).await
}

async fn run_repl_with(
    session: &mut PromptSession,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &Path,
) -> Result<()> {
    let mut frontend = crate::frontends::PrintFrontend::new();
    let interaction = InquireCommandInteraction;
    run_repl_with_frontend_and_interaction(session, registry, reader, history_path, &mut frontend, &interaction).await
}

#[cfg(test)]
async fn run_repl_with_frontend<O: crate::frontends::print::FrontendOutput>(
    session: &mut PromptSession,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &Path,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
) -> Result<()> {
    let interaction = InquireCommandInteraction;
    run_repl_with_frontend_and_interaction(session, registry, reader, history_path, frontend, &interaction).await
}

pub async fn run_repl_with_frontend_and_interaction<O: crate::frontends::print::FrontendOutput>(
    session: &mut PromptSession,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &Path,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
    interaction: &dyn CommandInteraction,
) -> Result<()> {
    let metadata = session.session().get_metadata().await;
    println!("{}", startup_banner("rusty-pi", session.model().id, &metadata.id));
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
            Err(error) => {
                eprintln!("[error] Input error: {error}");
                break;
            }
        };
        let input = line.trim().to_string();
        if input.is_empty() {
            continue;
        }
        reader.add_history_entry(&input);

        match resolve_input(&input, registry, session) {
            InputRoute::Command(invocation) => {
                let mut context = CommandContext::new(session, interaction, CancellationToken::new());
                let outcome: CommandOutcome = registry.dispatch(&invocation, &mut context).await?;
                if let Some(result) = outcome.result.as_ref() {
                    frontend.handle_command_result(result)?;
                }
                if outcome.control == crate::coding_agent::command::CommandControl::Quit {
                    break;
                }
            }
            InputRoute::AgentPrompt { expanded, .. } => {
                let _ = run_with_abort(session, &expanded, frontend).await?;
                println!();
            }
            InputRoute::UnknownSlash { name } => {
                frontend.handle_command_result(&crate::coding_agent::command::CommandResult::Error(format!(
                    "Unknown command '/{name}'. Type '/help' for available commands."
                )))?;
            }
        }
    }
    let _ = reader.save_history(history_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mock::MockProvider;
    use crate::ai::providers::Model;
    use crate::coding_agent::command::{InteractionResult, MockCommandInteraction};

    fn mock_session() -> PromptSession {
        PromptSession::new(
            Box::new(MockProvider::text("mock reply")),
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

    #[tokio::test]
    async fn repl_with_exit_and_quit_commands() {
        let registry = builtin_registry();
        for command in ["/exit", "/quit"] {
            let mut session = mock_session();
            let mut reader = crate::coding_agent::command::MockLineReader::new(vec![command.into()]);
            let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");
            run_repl_with(&mut session, &registry, &mut reader, &history)
                .await
                .unwrap();
            assert_eq!(reader.history, vec![command]);
        }
    }

    #[tokio::test]
    async fn repl_unknown_slash_does_not_call_agent() {
        let registry = builtin_registry();
        let mut session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/unknown".into(), "/exit".into()]);
        let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());
        run_repl_with_frontend(&mut session, &registry, &mut reader, &history, &mut frontend)
            .await
            .unwrap();
        assert!(frontend.output().stderr_str().contains("Unknown command"));
        assert!(session.agent().messages().await.is_empty());
    }

    #[tokio::test]
    async fn context_error_keeps_repl_alive_for_following_prompt() {
        let registry = builtin_registry();
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing/file");
        let mut session = mock_session();
        let before = session.system_prompt().to_string();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec![
            format!("/context {}", missing.display()),
            "ordinary prompt".into(),
            "/quit".into(),
        ]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());

        run_repl_with_frontend(
            &mut session,
            &registry,
            &mut reader,
            Path::new("/tmp/history"),
            &mut frontend,
        )
        .await
        .unwrap();

        let output = frontend.output();
        assert_eq!(output.stderr_str().matches("Cannot read").count(), 1);
        assert_eq!(
            reader.history,
            vec![
                format!("/context {}", missing.display()),
                "ordinary prompt".into(),
                "/quit".into(),
            ]
        );
        let messages = session.agent().messages().await;
        assert!(messages.iter().any(|message| matches!(
            message,
            crate::ai::types::AgentMessage::User(user)
                if matches!(&user.content, crate::ai::types::MessageContent::Text(text) if text == "ordinary prompt")
        )));
        assert!(!messages.iter().any(|message| matches!(
            message,
            crate::ai::types::AgentMessage::User(user)
                if matches!(&user.content, crate::ai::types::MessageContent::Text(text) if text.contains("/context"))
        )));
        assert_eq!(session.system_prompt(), before);
    }

    #[tokio::test]
    async fn context_invalid_utf8_keeps_repl_alive_for_following_prompt() {
        let registry = builtin_registry();
        let directory = tempfile::tempdir().unwrap();
        let invalid = directory.path().join("invalid.txt");
        tokio::fs::write(&invalid, [0xff, 0xfe, 0xfd]).await.unwrap();
        let mut session = mock_session();
        let before = session.system_prompt().to_string();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec![
            format!("/context {}", invalid.display()),
            "ordinary prompt".into(),
            "/quit".into(),
        ]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());

        run_repl_with_frontend(
            &mut session,
            &registry,
            &mut reader,
            Path::new("/tmp/history"),
            &mut frontend,
        )
        .await
        .unwrap();

        assert_eq!(frontend.output().stderr_str().matches("Cannot read").count(), 1);
        assert!(session.agent().messages().await.iter().any(|message| matches!(
            message,
            crate::ai::types::AgentMessage::User(user)
                if matches!(&user.content, crate::ai::types::MessageContent::Text(text) if text == "ordinary prompt")
        )));
        assert_eq!(session.system_prompt(), before);
    }

    #[tokio::test]
    async fn repl_model_cancel_continues() {
        let registry = builtin_registry();
        let mut session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/model".into(), "/exit".into()]);
        let interaction = MockCommandInteraction::new(vec![InteractionResult::Cancelled], vec![]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());
        run_repl_with_frontend_and_interaction(
            &mut session,
            &registry,
            &mut reader,
            Path::new("/tmp/history"),
            &mut frontend,
            &interaction,
        )
        .await
        .unwrap();
        assert!(reader.history.contains(&"/exit".into()));
    }

    #[tokio::test]
    async fn repl_command_output_failure_propagates() {
        let registry = builtin_registry();
        let mut session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/help".into()]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::FailingOutput);
        let error = run_repl_with_frontend(
            &mut session,
            &registry,
            &mut reader,
            Path::new("/tmp/history"),
            &mut frontend,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("stdout write failed"));
    }

    #[test]
    fn history_path_handles_home() {
        assert_eq!(
            history_path_for_home("/home/user"),
            PathBuf::from("/home/user/.pi/agent/repl-history.txt")
        );
    }
}
