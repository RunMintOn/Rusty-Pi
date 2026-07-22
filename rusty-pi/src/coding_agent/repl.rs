//! Thin blocking-line REPL frontend.
//!
//! The REPL owns rustyline and the `inquire` adapter, while command behavior,
//! routing, expansion, and rendering boundaries remain shared with the TUI.

use crate::ai::types::StopReason;
use crate::coding_agent::command::{
    CommandContext, CommandInteraction, CommandOutcome, CommandRegistry, InputRequest, InputRoute, InteractionResult,
    LineReader, SelectRequest, builtin_registry, resolve_input,
};
use crate::coding_agent::session_controller::{
    SessionControllerConnection, SessionControllerEventReceiver, SessionControllerHandle,
};
use crate::format::OutputFormatter;
use anyhow::Result;
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use std::path::{Path, PathBuf};

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
    pub connection: SessionControllerConnection,
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
    let RunConfig { prompt, connection } = config;
    let SessionControllerConnection {
        handle,
        mut events,
        task,
    } = connection;
    let mut user_cancelled = false;
    let result = match prompt {
        Some(prompt) => match run_single_shot(&handle, &mut events, &prompt).await {
            Err(error) => Err(error),
            Ok(outcome) => match outcome {
                crate::frontends::print::PrintRunOutcome::Finished(StopReason::Error) => {
                    Err(anyhow::anyhow!("Provider/model error"))
                }
                crate::frontends::print::PrintRunOutcome::Finished(StopReason::Aborted)
                | crate::frontends::print::PrintRunOutcome::Aborted => {
                    user_cancelled = true;
                    Ok(())
                }
                crate::frontends::print::PrintRunOutcome::Finished(_) => Ok(()),
                crate::frontends::print::PrintRunOutcome::Failed(error) => Err(anyhow::anyhow!("Run failed: {error}")),
                crate::frontends::print::PrintRunOutcome::Rejected(reason) => Err(anyhow::anyhow!(reason.to_string())),
            },
        },
        None => run_repl_with_controller(&handle, &mut events).await,
    };
    let shutdown = handle
        .shutdown()
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()));
    let joined = task.join().await.map_err(|error| anyhow::anyhow!(error.to_string()));
    let result = result.and(shutdown).and(joined);
    if user_cancelled {
        std::process::exit(130);
    }
    result
}

async fn run_with_abort<O: crate::frontends::print::FrontendOutput>(
    controller: &SessionControllerHandle,
    events: &mut SessionControllerEventReceiver,
    prompt: &str,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
) -> std::io::Result<crate::frontends::print::PrintRunOutcome> {
    crate::frontends::print::drive_print_run_with_interrupt(
        controller,
        events,
        prompt,
        frontend,
        tokio::signal::ctrl_c(),
    )
    .await
}

async fn run_single_shot(
    controller: &SessionControllerHandle,
    events: &mut SessionControllerEventReceiver,
    prompt: &str,
) -> Result<crate::frontends::print::PrintRunOutcome> {
    use crate::frontends::print::{PrintFrontend, RealOutput};
    let mut frontend = PrintFrontend::<RealOutput>::new();
    Ok(run_with_abort(controller, events, prompt, &mut frontend).await?)
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

async fn run_repl_with_controller(
    controller: &SessionControllerHandle,
    events: &mut SessionControllerEventReceiver,
) -> Result<()> {
    let hist_path = history_path();
    if let Some(parent) = hist_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let editor = DefaultEditor::new().map_err(|error| anyhow::anyhow!("Failed to create REPL editor: {error}"))?;
    let mut reader = RustylineReader::new(editor);
    let _ = reader.inner.load_history(&hist_path);
    let registry = builtin_registry();
    let mut frontend = crate::frontends::PrintFrontend::new();
    let interaction = InquireCommandInteraction;
    run_repl_with_frontend_and_interaction(
        controller,
        events,
        &registry,
        &mut reader,
        &hist_path,
        &mut frontend,
        &interaction,
    )
    .await
}

pub async fn run_repl_with_frontend_and_interaction<O: crate::frontends::print::FrontendOutput>(
    controller: &SessionControllerHandle,
    events: &mut SessionControllerEventReceiver,
    registry: &CommandRegistry,
    reader: &mut dyn LineReader,
    history_path: &Path,
    frontend: &mut crate::frontends::print::PrintFrontend<O>,
    interaction: &dyn CommandInteraction,
) -> Result<()> {
    let snapshot = controller
        .snapshot()
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    println!(
        "{}",
        startup_banner(
            "rusty-pi",
            &snapshot.model.id,
            snapshot.session_id.as_deref().unwrap_or("")
        )
    );
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

        match resolve_input(&input, registry) {
            InputRoute::Command(invocation) => {
                let mut context =
                    CommandContext::new(controller, interaction, tokio_util::sync::CancellationToken::new());
                let outcome: CommandOutcome = registry.dispatch(&invocation, &mut context).await?;
                if let Some(result) = outcome.result.as_ref() {
                    frontend.handle_command_result(result)?;
                }
                if outcome.control == crate::coding_agent::command::CommandControl::Quit {
                    break;
                }
            }
            InputRoute::AgentPrompt { original } => {
                let _ = run_with_abort(controller, events, &original, frontend).await?;
                println!();
            }
        }
    }
    let _ = reader.save_history(history_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::command::{InteractionResult, MockCommandInteraction};

    fn mock_session() -> SessionControllerConnection {
        crate::test_support::mock_controller_connection()
    }

    async fn run_repl_with(
        connection: SessionControllerConnection,
        registry: &CommandRegistry,
        reader: &mut dyn LineReader,
        history_path: &Path,
    ) -> Result<()> {
        let mut frontend = crate::frontends::PrintFrontend::new();
        let interaction = InquireCommandInteraction;
        run_repl_with_test_interaction(connection, registry, reader, history_path, &mut frontend, &interaction).await
    }

    async fn run_repl_with_frontend<O: crate::frontends::print::FrontendOutput>(
        connection: SessionControllerConnection,
        registry: &CommandRegistry,
        reader: &mut dyn LineReader,
        history_path: &Path,
        frontend: &mut crate::frontends::print::PrintFrontend<O>,
    ) -> Result<()> {
        let interaction = InquireCommandInteraction;
        run_repl_with_test_interaction(connection, registry, reader, history_path, frontend, &interaction).await
    }

    async fn run_repl_with_test_interaction<O: crate::frontends::print::FrontendOutput>(
        connection: SessionControllerConnection,
        registry: &CommandRegistry,
        reader: &mut dyn LineReader,
        history_path: &Path,
        frontend: &mut crate::frontends::print::PrintFrontend<O>,
        interaction: &dyn CommandInteraction,
    ) -> Result<()> {
        let SessionControllerConnection {
            handle,
            mut events,
            task,
        } = connection;
        let result = run_repl_with_frontend_and_interaction(
            &handle,
            &mut events,
            registry,
            reader,
            history_path,
            frontend,
            interaction,
        )
        .await;
        let shutdown = handle
            .shutdown()
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()));
        let joined = task.join().await.map_err(|error| anyhow::anyhow!(error.to_string()));
        result.and(shutdown).and(joined)
    }

    #[tokio::test]
    async fn repl_with_exit_and_quit_commands() {
        let registry = builtin_registry();
        for command in ["/exit", "/quit"] {
            let session = mock_session();
            let mut reader = crate::coding_agent::command::MockLineReader::new(vec![command.into()]);
            let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");
            run_repl_with(session, &registry, &mut reader, &history).await.unwrap();
            assert_eq!(reader.history, vec![command]);
        }
    }

    #[tokio::test]
    async fn repl_unknown_slash_does_not_call_agent() {
        let registry = builtin_registry();
        let session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/unknown".into(), "/exit".into()]);
        let history = PathBuf::from("/tmp/.pi/agent/repl-history.txt");
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());
        run_repl_with_frontend(session, &registry, &mut reader, &history, &mut frontend)
            .await
            .unwrap();
        assert!(frontend.output().stderr_str().contains("Unknown command"));
    }

    #[tokio::test]
    async fn context_error_keeps_repl_alive_for_following_prompt() {
        let registry = builtin_registry();
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing/file");
        let session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec![
            format!("/context {}", missing.display()),
            "ordinary prompt".into(),
            "/quit".into(),
        ]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());

        run_repl_with_frontend(
            session,
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
        assert!(output.stdout_str().contains("mock reply"));
    }

    #[tokio::test]
    async fn context_invalid_utf8_keeps_repl_alive_for_following_prompt() {
        let registry = builtin_registry();
        let directory = tempfile::tempdir().unwrap();
        let invalid = directory.path().join("invalid.txt");
        tokio::fs::write(&invalid, [0xff, 0xfe, 0xfd]).await.unwrap();
        let session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec![
            format!("/context {}", invalid.display()),
            "ordinary prompt".into(),
            "/quit".into(),
        ]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());

        run_repl_with_frontend(
            session,
            &registry,
            &mut reader,
            Path::new("/tmp/history"),
            &mut frontend,
        )
        .await
        .unwrap();

        assert_eq!(frontend.output().stderr_str().matches("Cannot read").count(), 1);
        assert!(frontend.output().stdout_str().contains("mock reply"));
    }

    #[tokio::test]
    async fn repl_model_cancel_continues() {
        let registry = builtin_registry();
        let session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/model".into(), "/exit".into()]);
        let interaction = MockCommandInteraction::new(vec![InteractionResult::Cancelled], vec![]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::MemoryOutput::new());
        run_repl_with_test_interaction(
            session,
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
        let session = mock_session();
        let mut reader = crate::coding_agent::command::MockLineReader::new(vec!["/help".into()]);
        let mut frontend = crate::frontends::PrintFrontend::with_output(crate::frontends::print::FailingOutput);
        let error = run_repl_with_frontend(
            session,
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
