//! Test-only adapters shared by integration tests.
//!
//! This module is deliberately outside frontend production modules. It keeps
//! the pre-controller Agent seam available for focused Agent/PrintFrontend
//! compatibility tests without making that seam part of the frontend runtime.

use crate::agent::events::{AgentEvent, AgentRunError, AgentRunPhase};
use crate::ai::providers::Model;
use crate::coding_agent::prompt_session::PromptSession;
use crate::coding_agent::session_controller::{
    ContextChangeResult, ControllerActivity, ControllerRequestError, ModelChangeResult, ModelSummary,
    SessionController, SessionControllerConnection, SessionControllerHandle, SessionSnapshot, SessionTreeSnapshot,
};
use crate::frontends::print::{FrontendOutput, PrintFrontend, PrintRunOutcome};
use std::io;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(crate) type TestResourceOwner = PromptSession;

pub(crate) async fn drive_print_run<O: FrontendOutput>(
    agent: &mut crate::agent::engine::Agent,
    prompt: &str,
    frontend: &mut PrintFrontend<O>,
    run_token: CancellationToken,
) -> io::Result<PrintRunOutcome> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(256);
    agent.set_event_sender(event_tx);
    agent.set_abort_flag(run_token.clone());
    let run = agent.run(prompt);
    tokio::pin!(run);
    let mut outcome = None;
    let mut output_error = None;
    loop {
        tokio::select! {
            result = &mut run => {
                while let Ok(event) = event_rx.try_recv() {
                    if output_error.is_none()
                        && let Err(error) = frontend.handle_event(&event)
                    {
                        run_token.cancel();
                        output_error = Some(error);
                    }
                    record_outcome(&mut outcome, event);
                }
                if outcome.is_none() && let Err(error) = result {
                    let failure = AgentRunError {
                        phase: AgentRunPhase::AgentLoop,
                        message: error.to_string(),
                    };
                    if output_error.is_none() {
                        frontend.report_run_error(&failure)?;
                    }
                    outcome = Some(PrintRunOutcome::Failed(failure));
                }
                break;
            }
            event = event_rx.recv() => {
                let Some(event) = event else { break };
                if output_error.is_none()
                    && let Err(error) = frontend.handle_event(&event)
                {
                    run_token.cancel();
                    output_error = Some(error);
                }
                record_outcome(&mut outcome, event);
            }
        }
    }
    if let Some(error) = output_error {
        Err(error)
    } else {
        Ok(outcome.unwrap_or(PrintRunOutcome::Aborted))
    }
}

fn record_outcome(outcome: &mut Option<PrintRunOutcome>, event: AgentEvent) {
    *outcome = match event {
        AgentEvent::RunFinished { stop_reason, .. } => Some(PrintRunOutcome::Finished(stop_reason)),
        AgentEvent::RunAborted { .. } => Some(PrintRunOutcome::Aborted),
        AgentEvent::RunFailed { error, .. } => Some(PrintRunOutcome::Failed(error)),
        _ => outcome.take(),
    };
}

#[cfg(test)]
pub(crate) enum CommandController<'a> {
    Handle(&'a SessionControllerHandle),
    Prompt(&'a mut TestResourceOwner),
}

#[cfg(test)]
impl<'a> CommandController<'a> {
    pub(crate) async fn snapshot(&self) -> Result<SessionSnapshot, ControllerRequestError> {
        match self {
            Self::Handle(handle) => handle.snapshot().await,
            Self::Prompt(session) => {
                let metadata = session.session_metadata().await;
                Ok(SessionSnapshot {
                    activity: ControllerActivity::Idle,
                    session_id: Some(metadata.id),
                    model: ModelSummary::from(session.model()),
                    message_count: session.message_count().await,
                    current_request_id: None,
                    current_run_id: None,
                    context_files: session.context_paths(),
                    skill_names: session.skill_names(),
                    template_names: session.template_names(),
                    agent_dir: session.agent_dir().to_path_buf(),
                    cwd: session.cwd().to_path_buf(),
                })
            }
        }
    }

    pub(crate) async fn list_models(&self) -> Result<Vec<ModelSummary>, ControllerRequestError> {
        match self {
            Self::Handle(handle) => handle.list_models().await,
            Self::Prompt(session) => Ok(session.list_models().iter().map(ModelSummary::from).collect()),
        }
    }

    pub(crate) async fn switch_model(&mut self, model_id: String) -> Result<ModelChangeResult, ControllerRequestError> {
        match self {
            Self::Handle(handle) => handle.switch_model(model_id).await,
            Self::Prompt(session) => {
                let model = session
                    .list_models()
                    .into_iter()
                    .find(|model| model.id == model_id)
                    .ok_or_else(|| ControllerRequestError::InvalidModel(model_id.clone()))?;
                let summary = ModelSummary::from(&model);
                if session.model().id == model.id {
                    Ok(ModelChangeResult {
                        model: summary,
                        changed: false,
                    })
                } else {
                    session.switch_model(model);
                    Ok(ModelChangeResult {
                        model: summary,
                        changed: true,
                    })
                }
            }
        }
    }

    pub(crate) async fn add_context(
        &mut self,
        path: PathBuf,
        content: String,
    ) -> Result<ContextChangeResult, ControllerRequestError> {
        match self {
            Self::Handle(handle) => handle.add_context(path, content).await,
            Self::Prompt(session) => {
                session.add_context_file(path.clone(), content);
                Ok(ContextChangeResult { path })
            }
        }
    }

    pub(crate) async fn session_tree(&self) -> Result<SessionTreeSnapshot, ControllerRequestError> {
        match self {
            Self::Handle(handle) => handle.session_tree().await,
            Self::Prompt(session) => Ok(SessionTreeSnapshot::from_entries(session.session_entries().await)),
        }
    }
}

#[cfg(test)]
pub(crate) trait CommandContextOwner<'a> {
    fn into_controller(self) -> CommandController<'a>;
}

#[cfg(test)]
impl<'a> CommandContextOwner<'a> for &'a SessionControllerHandle {
    fn into_controller(self) -> CommandController<'a> {
        CommandController::Handle(self)
    }
}

#[cfg(test)]
impl<'a> CommandContextOwner<'a> for &'a mut TestResourceOwner {
    fn into_controller(self) -> CommandController<'a> {
        CommandController::Prompt(self)
    }
}

#[cfg(test)]
pub(crate) fn mock_controller_connection() -> SessionControllerConnection {
    let provider = crate::ai::mock::MockProvider::text("mock reply");
    let session = TestResourceOwner::new(
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
    SessionController::spawn(session)
}
