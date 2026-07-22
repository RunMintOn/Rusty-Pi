//! A frontend-neutral, task-owned session lifecycle.
//!
//! `SessionController` is deliberately a deep module: callers submit business
//! requests and consume owned values/events, while the task exclusively owns
//! PromptSession, Agent, session storage, the active run future, and its
//! cancellation token.  No frontend receives an internal object reference.

use crate::agent::events::{AgentEvent, RunId};
use crate::agent::session::types::SessionTreeEntry;
use crate::ai::providers::Model;
use crate::coding_agent::prompt_session::{PromptExpansion, PromptSession};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const CONTROLLER_EVENT_CAPACITY: usize = 256;
const AGENT_EVENT_CAPACITY: usize = 256;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Independent identifier for one frontend prompt submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request-{}", self.0)
    }
}

/// Observable controller activity. The active run remains owned by the task
/// until its future settles, even after a terminal AgentEvent was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControllerActivity {
    Idle,
    Running { request_id: RequestId, run_id: RunId },
    Cancelling { request_id: RequestId, run_id: RunId },
    ShuttingDown,
    Stopped,
}

/// Model data safe to copy across the controller seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSummary {
    pub id: String,
    pub api: String,
}

impl From<&Model> for ModelSummary {
    fn from(model: &Model) -> Self {
        Self {
            id: model.id.to_string(),
            api: model.api.to_string(),
        }
    }
}

/// Read-only session data. Every collection is owned by the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub activity: ControllerActivity,
    pub session_id: Option<String>,
    pub model: ModelSummary,
    pub message_count: usize,
    pub current_request_id: Option<RequestId>,
    pub current_run_id: Option<RunId>,
    pub context_files: Vec<PathBuf>,
    pub skill_names: Vec<String>,
    pub template_names: Vec<String>,
    /// Used by the independent session catalog command; it is a copied path,
    /// never a storage handle.
    pub agent_dir: PathBuf,
    pub cwd: PathBuf,
}

/// Owned tree result returned by `/tree`.
#[derive(Debug, Clone)]
pub struct SessionTreeSnapshot {
    entries: Vec<SessionTreeEntry>,
}

impl SessionTreeSnapshot {
    #[cfg(test)]
    pub(crate) fn from_entries(entries: Vec<SessionTreeEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[SessionTreeEntry] {
        &self.entries
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRejection {
    Empty,
    UnknownSlash { name: String },
    ReservedCommand { name: String },
}

impl std::fmt::Display for PromptRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "Prompt is empty"),
            Self::UnknownSlash { name } => {
                write!(f, "Unknown command '/{name}'. Type '/help' for available commands.")
            }
            Self::ReservedCommand { name } => {
                write!(
                    f,
                    "'/{name}' is a built-in command and must be routed through the command registry"
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSubmission {
    Accepted {
        request_id: RequestId,
        run_id: RunId,
        original: String,
    },
    Rejected {
        request_id: RequestId,
        reason: PromptRejection,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelResult {
    CancellationRequested { request_id: RequestId, run_id: RunId },
    AlreadyCancelling { request_id: RequestId, run_id: RunId },
    NothingRunning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChangeResult {
    pub model: ModelSummary,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextChangeResult {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerOperation {
    Spawn,
    Prompt,
    Cancel,
    Snapshot,
    Models,
    ModelChange,
    Context,
    SessionTree,
    Shutdown,
    RunSettlement,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ControllerRequestError {
    #[error("session controller channel is closed")]
    Closed,
    #[error("session controller has stopped")]
    Stopped,
    #[error("session controller is busy: {0:?}")]
    Busy(ControllerActivity),
    #[error("invalid model: {0}")]
    InvalidModel(String),
    #[error("invalid prompt: {0}")]
    InvalidPrompt(String),
    #[error("session operation failed: {0}")]
    Session(String),
    #[error("controller internal error: {0}")]
    Internal(String),
}

/// Events sent through the one active frontend receiver.
#[derive(Debug)]
pub enum SessionControllerEvent {
    StateChanged(ControllerActivity),
    PromptAccepted {
        request_id: RequestId,
        run_id: RunId,
        original: String,
    },
    PromptRejected {
        request_id: RequestId,
        reason: PromptRejection,
    },
    Agent {
        request_id: RequestId,
        event: AgentEvent,
    },
    ControllerError {
        operation: ControllerOperation,
        message: String,
    },
    Stopped,
}

pub type SessionControllerEventReceiver = mpsc::UnboundedReceiver<SessionControllerEvent>;

type ControllerEventSender = mpsc::UnboundedSender<SessionControllerEvent>;
type ControllerReply<T> = oneshot::Sender<Result<T, ControllerRequestError>>;

enum ControllerRequest {
    SubmitPrompt {
        input: String,
        reply: ControllerReply<PromptSubmission>,
    },
    CancelCurrent {
        reply: ControllerReply<CancelResult>,
    },
    Snapshot {
        reply: ControllerReply<SessionSnapshot>,
    },
    ListModels {
        reply: ControllerReply<Vec<ModelSummary>>,
    },
    SwitchModel {
        model_id: String,
        reply: ControllerReply<ModelChangeResult>,
    },
    AddContext {
        path: PathBuf,
        content: String,
        reply: ControllerReply<ContextChangeResult>,
    },
    SessionTree {
        reply: ControllerReply<SessionTreeSnapshot>,
    },
    Shutdown {
        reply: ControllerReply<()>,
    },
}

/// Cloneable request-side handle. It contains no session or Agent reference.
#[derive(Clone)]
pub struct SessionControllerHandle {
    requests: mpsc::Sender<ControllerRequest>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl SessionControllerHandle {
    pub async fn submit_prompt(&self, input: impl Into<String>) -> Result<PromptSubmission, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(
            ControllerRequest::SubmitPrompt {
                input: input.into(),
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn cancel_current(&self) -> Result<CancelResult, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(ControllerRequest::CancelCurrent { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn snapshot(&self) -> Result<SessionSnapshot, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(ControllerRequest::Snapshot { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn list_models(&self) -> Result<Vec<ModelSummary>, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(ControllerRequest::ListModels { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn switch_model(&self, model_id: impl Into<String>) -> Result<ModelChangeResult, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(
            ControllerRequest::SwitchModel {
                model_id: model_id.into(),
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn add_context(
        &self,
        path: PathBuf,
        content: String,
    ) -> Result<ContextChangeResult, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(
            ControllerRequest::AddContext {
                path,
                content,
                reply: reply_tx,
            },
            reply_rx,
        )
        .await
    }

    pub async fn session_tree(&self) -> Result<SessionTreeSnapshot, ControllerRequestError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(ControllerRequest::SessionTree { reply: reply_tx }, reply_rx)
            .await
    }

    pub async fn shutdown(&self) -> Result<(), ControllerRequestError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(ControllerRequestError::Stopped);
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.send(ControllerRequest::Shutdown { reply: reply_tx }, reply_rx)
            .await
    }

    async fn send<T>(
        &self,
        request: ControllerRequest,
        reply: oneshot::Receiver<Result<T, ControllerRequestError>>,
    ) -> Result<T, ControllerRequestError> {
        self.requests
            .send(request)
            .await
            .map_err(|_| ControllerRequestError::Closed)?;
        reply.await.map_err(|_| ControllerRequestError::Closed)?
    }
}

/// Explicit owner of the controller task settlement handle.
pub struct SessionControllerTaskHandle {
    join: Option<tokio::task::JoinHandle<()>>,
}

impl SessionControllerTaskHandle {
    pub async fn join(mut self) -> Result<(), tokio::task::JoinError> {
        match self.join.take() {
            Some(join) => join.await,
            None => Ok(()),
        }
    }
}

impl Drop for SessionControllerTaskHandle {
    fn drop(&mut self) {
        if let Some(join) = self.join.take()
            && !join.is_finished()
        {
            join.abort();
        }
    }
}

/// The three objects needed to use and settle a controller.
pub struct SessionControllerConnection {
    pub handle: SessionControllerHandle,
    pub events: SessionControllerEventReceiver,
    pub task: SessionControllerTaskHandle,
}

/// Spawn the permanent task and move the PromptSession into it.
pub struct SessionController;

impl SessionController {
    pub fn spawn(prompt_session: PromptSession) -> SessionControllerConnection {
        let (request_tx, request_rx) = mpsc::channel(CONTROLLER_EVENT_CAPACITY);
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = SessionControllerHandle {
            requests: request_tx,
            stopped: stopped.clone(),
        };
        let join = tokio::spawn(run_controller(prompt_session, request_rx, event_tx, stopped));
        SessionControllerConnection {
            handle,
            events: event_rx,
            task: SessionControllerTaskHandle { join: Some(join) },
        }
    }
}

#[derive(Clone)]
struct SnapshotData {
    session_id: Option<String>,
    model: ModelSummary,
    message_count: usize,
    context_files: Vec<PathBuf>,
    skill_names: Vec<String>,
    template_names: Vec<String>,
    agent_dir: PathBuf,
    cwd: PathBuf,
    models: Vec<ModelSummary>,
}

impl SnapshotData {
    async fn load(session: &PromptSession) -> Self {
        let metadata = session.session_metadata().await;
        let models = session.list_models();
        Self {
            session_id: Some(metadata.id),
            model: ModelSummary::from(session.model()),
            message_count: session.message_count().await,
            context_files: session.context_paths(),
            skill_names: session.skill_names(),
            template_names: session.template_names(),
            agent_dir: session.agent_dir().to_path_buf(),
            cwd: session.cwd().to_path_buf(),
            models: models.iter().map(ModelSummary::from).collect(),
        }
    }

    fn snapshot(&self, activity: ControllerActivity) -> SessionSnapshot {
        let (current_request_id, current_run_id) = match activity {
            ControllerActivity::Running { request_id, run_id }
            | ControllerActivity::Cancelling { request_id, run_id } => (Some(request_id), Some(run_id)),
            _ => (None, None),
        };
        SessionSnapshot {
            activity,
            session_id: self.session_id.clone(),
            model: self.model.clone(),
            message_count: self.message_count,
            current_request_id,
            current_run_id,
            context_files: self.context_files.clone(),
            skill_names: self.skill_names.clone(),
            template_names: self.template_names.clone(),
            agent_dir: self.agent_dir.clone(),
            cwd: self.cwd.clone(),
        }
    }
}

struct ActiveRunMeta {
    request_id: RequestId,
    run_id: RunId,
    cancellation: CancellationToken,
    terminal_seen: bool,
}

struct StartRun {
    request_id: RequestId,
    run_id: RunId,
    expanded: String,
    cancellation: CancellationToken,
}

type RunFuture = Pin<Box<dyn Future<Output = (PromptSession, anyhow::Result<()>)> + Send>>;

enum ControllerPoll {
    Request(Option<ControllerRequest>),
    Agent(Option<AgentEvent>),
    Settled(Box<(PromptSession, anyhow::Result<()>)>),
}

async fn run_controller(
    mut prompt_session: PromptSession,
    mut requests: mpsc::Receiver<ControllerRequest>,
    events: ControllerEventSender,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    let (agent_event_tx, mut agent_events) = mpsc::channel(AGENT_EVENT_CAPACITY);
    prompt_session.set_event_sender(agent_event_tx);
    let mut cache = SnapshotData::load(&prompt_session).await;
    let mut prompt_session = Some(prompt_session);
    let mut activity = ControllerActivity::Idle;
    let mut run_future: Option<RunFuture> = None;
    let mut active: Option<ActiveRunMeta> = None;
    let mut shutdown_requested = false;
    let mut shutdown_replies: Vec<ControllerReply<()>> = Vec::new();
    emit_state(&events, activity.clone());

    loop {
        if shutdown_requested && active.is_none() {
            activity = ControllerActivity::Stopped;
            stopped.store(true, Ordering::Release);
            emit_state(&events, activity.clone());
            let _ = events.send(SessionControllerEvent::Stopped);
            for reply in shutdown_replies.drain(..) {
                let _ = reply.send(Ok(()));
            }
            break;
        }

        if active.is_none() {
            let request = requests.recv().await;
            let Some(request) = request else {
                shutdown_requested = true;
                activity = ControllerActivity::ShuttingDown;
                emit_state(&events, activity.clone());
                continue;
            };
            let Some(prompt_session_ref) = prompt_session.as_mut() else {
                emit_controller_error(&events, ControllerOperation::Spawn, "session ownership is missing");
                shutdown_requested = true;
                continue;
            };
            let start = handle_idle_request(
                request,
                prompt_session_ref,
                &events,
                &mut cache,
                &mut activity,
                &mut shutdown_requested,
                &mut shutdown_replies,
            )
            .await;
            if let Some(start) = start {
                let Some(mut owned_session) = prompt_session.take() else {
                    emit_controller_error(&events, ControllerOperation::Spawn, "session ownership was lost");
                    shutdown_requested = true;
                    continue;
                };
                active = Some(ActiveRunMeta {
                    request_id: start.request_id,
                    run_id: start.run_id,
                    cancellation: start.cancellation,
                    terminal_seen: false,
                });
                run_future = Some(Box::pin(async move {
                    let result = owned_session.run_expanded(start.expanded, start.run_id).await;
                    (owned_session, result)
                }));
            }
            continue;
        }

        let Some(future) = run_future.as_mut() else {
            emit_controller_error(&events, ControllerOperation::RunSettlement, "active run has no future");
            shutdown_requested = true;
            continue;
        };
        let poll = tokio::select! {
            biased;
            request = requests.recv() => ControllerPoll::Request(request),
            event = agent_events.recv() => ControllerPoll::Agent(event),
            result = future => ControllerPoll::Settled(Box::new(result)),
        };

        match poll {
            ControllerPoll::Request(Some(request)) => {
                handle_active_request(
                    request,
                    &events,
                    &mut activity,
                    match active.as_mut() {
                        Some(active) => active,
                        None => continue,
                    },
                    &cache,
                    &mut shutdown_requested,
                    &mut shutdown_replies,
                );
            }
            ControllerPoll::Request(None) => {
                shutdown_requested = true;
                if let Some(run) = active.as_mut() {
                    run.cancellation.cancel();
                }
                activity = ControllerActivity::ShuttingDown;
                emit_state(&events, activity.clone());
            }
            ControllerPoll::Agent(Some(event)) => {
                if let Some(run) = active.as_mut() {
                    forward_agent_event(&events, run, event);
                }
            }
            ControllerPoll::Agent(None) => {
                emit_controller_error(
                    &events,
                    ControllerOperation::RunSettlement,
                    "agent event channel closed",
                );
                shutdown_requested = true;
                if let Some(run) = active.as_mut() {
                    run.cancellation.cancel();
                }
                activity = ControllerActivity::ShuttingDown;
                emit_state(&events, activity.clone());
            }
            ControllerPoll::Settled(result) => {
                // Dropping the future before touching PromptSession releases
                // its mutable borrow and proves the run has settled.
                run_future = None;
                let terminal_seen = active.as_ref().is_some_and(|run| run.terminal_seen);
                if let Some(mut run) = active.take() {
                    while let Ok(event) = agent_events.try_recv() {
                        forward_agent_event(&events, &mut run, event);
                    }
                }
                let (restored_session, result) = *result;
                prompt_session = Some(restored_session);
                cache = match prompt_session.as_ref() {
                    Some(session) => SnapshotData::load(session).await,
                    None => {
                        emit_controller_error(&events, ControllerOperation::RunSettlement, "session was not restored");
                        shutdown_requested = true;
                        continue;
                    }
                };
                if let Err(error) = result
                    && !terminal_seen
                {
                    emit_controller_error(&events, ControllerOperation::RunSettlement, error.to_string());
                }
                if shutdown_requested {
                    active = None;
                } else {
                    activity = ControllerActivity::Idle;
                    emit_state(&events, activity.clone());
                }
            }
        }
    }
}

async fn handle_idle_request(
    request: ControllerRequest,
    prompt_session: &mut PromptSession,
    events: &ControllerEventSender,
    cache: &mut SnapshotData,
    activity: &mut ControllerActivity,
    shutdown_requested: &mut bool,
    shutdown_replies: &mut Vec<ControllerReply<()>>,
) -> Option<StartRun> {
    match request {
        ControllerRequest::SubmitPrompt { input, reply } => {
            let request_id = next_request_id();
            let expanded = match classify_prompt(prompt_session, &input) {
                Ok(expanded) => expanded,
                Err(reason) => {
                    let submission = PromptSubmission::Rejected {
                        request_id,
                        reason: reason.clone(),
                    };
                    let _ = reply.send(Ok(submission));
                    let _ = events.send(SessionControllerEvent::PromptRejected { request_id, reason });
                    return None;
                }
            };
            let run_id = prompt_session.allocate_run_id();
            let cancellation = CancellationToken::new();
            prompt_session.set_abort_token(cancellation.clone());
            let original = input.clone();
            let submission = PromptSubmission::Accepted {
                request_id,
                run_id,
                original: original.clone(),
            };
            *activity = ControllerActivity::Running { request_id, run_id };
            cache.message_count = cache.message_count.saturating_add(1);
            let _ = reply.send(Ok(submission));
            let _ = events.send(SessionControllerEvent::PromptAccepted {
                request_id,
                run_id,
                original,
            });
            emit_state(events, activity.clone());
            Some(StartRun {
                request_id,
                run_id,
                expanded,
                cancellation,
            })
        }
        ControllerRequest::CancelCurrent { reply } => {
            let _ = reply.send(Ok(CancelResult::NothingRunning));
            None
        }
        ControllerRequest::Snapshot { reply } => {
            let _ = reply.send(Ok(cache.snapshot(activity.clone())));
            None
        }
        ControllerRequest::ListModels { reply } => {
            let _ = reply.send(Ok(cache.models.clone()));
            None
        }
        ControllerRequest::SwitchModel { model_id, reply } => {
            let Some(model_index) = cache.models.iter().position(|model| model.id == model_id) else {
                let _ = reply.send(Err(ControllerRequestError::InvalidModel(model_id)));
                return None;
            };
            let model = cache.models[model_index].clone();
            if cache.model == model {
                let _ = reply.send(Ok(ModelChangeResult { model, changed: false }));
                return None;
            }
            let Some(agent_model) = prompt_session
                .list_models()
                .into_iter()
                .find(|candidate| candidate.id == model.id)
            else {
                let _ = reply.send(Err(ControllerRequestError::InvalidModel(model.id)));
                return None;
            };
            prompt_session.switch_model(agent_model);
            cache.model = model.clone();
            let _ = reply.send(Ok(ModelChangeResult { model, changed: true }));
            None
        }
        ControllerRequest::AddContext { path, content, reply } => {
            prompt_session.add_context_file(path.clone(), content);
            cache.context_files.push(path.clone());
            let _ = reply.send(Ok(ContextChangeResult { path }));
            None
        }
        ControllerRequest::SessionTree { reply } => {
            let entries = prompt_session.session_entries().await;
            let _ = reply.send(Ok(SessionTreeSnapshot { entries }));
            None
        }
        ControllerRequest::Shutdown { reply } => {
            *shutdown_requested = true;
            *activity = ControllerActivity::ShuttingDown;
            emit_state(events, activity.clone());
            shutdown_replies.push(reply);
            None
        }
    }
}

fn handle_active_request(
    request: ControllerRequest,
    events: &ControllerEventSender,
    activity: &mut ControllerActivity,
    active: &mut ActiveRunMeta,
    cache: &SnapshotData,
    shutdown_requested: &mut bool,
    shutdown_replies: &mut Vec<ControllerReply<()>>,
) {
    match request {
        ControllerRequest::SubmitPrompt { reply, .. } => {
            let _ = reply.send(Err(ControllerRequestError::Busy(activity.clone())));
        }
        ControllerRequest::CancelCurrent { reply } => {
            if *shutdown_requested
                || matches!(activity, ControllerActivity::ShuttingDown)
                || active.cancellation.is_cancelled()
            {
                let _ = reply.send(Ok(CancelResult::AlreadyCancelling {
                    request_id: active.request_id,
                    run_id: active.run_id,
                }));
            } else {
                active.cancellation.cancel();
                *activity = ControllerActivity::Cancelling {
                    request_id: active.request_id,
                    run_id: active.run_id,
                };
                let _ = reply.send(Ok(CancelResult::CancellationRequested {
                    request_id: active.request_id,
                    run_id: active.run_id,
                }));
                emit_state(events, activity.clone());
            }
        }
        ControllerRequest::Snapshot { reply } => {
            let _ = reply.send(Ok(cache.snapshot(activity.clone())));
        }
        ControllerRequest::ListModels { reply } => {
            let _ = reply.send(Err(ControllerRequestError::Busy(activity.clone())));
        }
        ControllerRequest::SwitchModel { reply, .. } => {
            let _ = reply.send(Err(ControllerRequestError::Busy(activity.clone())));
        }
        ControllerRequest::AddContext { reply, .. } => {
            let _ = reply.send(Err(ControllerRequestError::Busy(activity.clone())));
        }
        ControllerRequest::SessionTree { reply } => {
            let _ = reply.send(Err(ControllerRequestError::Busy(activity.clone())));
        }
        ControllerRequest::Shutdown { reply } => {
            *shutdown_requested = true;
            active.cancellation.cancel();
            *activity = ControllerActivity::ShuttingDown;
            emit_state(events, activity.clone());
            shutdown_replies.push(reply);
        }
    }
}

fn forward_agent_event(events: &ControllerEventSender, active: &mut ActiveRunMeta, event: AgentEvent) {
    if is_terminal(&event) {
        active.terminal_seen = true;
    }
    let _ = events.send(SessionControllerEvent::Agent {
        request_id: active.request_id,
        event,
    });
}

fn is_terminal(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::RunFinished { .. } | AgentEvent::RunAborted { .. } | AgentEvent::RunFailed { .. }
    )
}

fn classify_prompt(session: &PromptSession, input: &str) -> Result<String, PromptRejection> {
    if input.trim().is_empty() {
        return Err(PromptRejection::Empty);
    }
    let normalized = input.trim_start();
    if let Some(name) = slash_name(normalized) {
        if is_reserved_command(name) {
            return Err(PromptRejection::ReservedCommand { name: name.to_string() });
        }
        return match session.try_expand_prompt_command(normalized) {
            PromptExpansion::Expanded(expanded) => Ok(expanded),
            PromptExpansion::NotMatched => Err(PromptRejection::UnknownSlash { name: name.to_string() }),
        };
    }
    Ok(input.to_string())
}

fn slash_name(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('/')?;
    Some(rest.split(char::is_whitespace).next().unwrap_or(rest))
}

fn is_reserved_command(name: &str) -> bool {
    matches!(
        name,
        "help" | "exit" | "quit" | "model" | "context" | "session" | "tree" | "list-sessions"
    )
}

fn next_request_id() -> RequestId {
    RequestId(NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
}

fn emit_state(events: &ControllerEventSender, activity: ControllerActivity) {
    let _ = events.send(SessionControllerEvent::StateChanged(activity));
}

fn emit_controller_error(events: &ControllerEventSender, operation: ControllerOperation, message: impl Into<String>) {
    let _ = events.send(SessionControllerEvent::ControllerError {
        operation,
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionContext};
    use crate::ai::mock::{MockProvider, MockStep};
    use crate::ai::providers::{Model, ProviderApi, ProviderRequestContext, ProviderStream};
    use crate::ai::types::{Content, Tool};
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use tokio::sync::{Notify, mpsc};
    use tokio::time::{Duration, timeout};

    fn session(provider: Box<dyn ProviderApi>, tools: Vec<Box<dyn AgentTool>>) -> PromptSession {
        PromptSession::new(
            provider,
            Model {
                id: "mock",
                api: "mock",
            },
            tools,
            Path::new("/tmp").to_path_buf(),
            Path::new("/tmp/.pi/agent").to_path_buf(),
            vec![],
            vec![],
            false,
            None,
            vec![],
        )
    }

    fn mock_session(provider: MockProvider) -> PromptSession {
        session(Box::new(provider), vec![])
    }

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "echo"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    #[async_trait]
    impl AgentTool for EchoTool {
        fn label(&self) -> &str {
            "echo"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "echoed".into() }],
                ..Default::default()
            })
        }
    }

    struct StalledProvider {
        started: Arc<Notify>,
        finished: Arc<AtomicBool>,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ProviderApi for StalledProvider {
        async fn stream(
            &self,
            _model: &Model,
            _messages: &[crate::ai::types::AgentMessage],
            _tools: &[&dyn Tool],
            _system_prompt: Option<&str>,
            context: ProviderRequestContext,
        ) -> anyhow::Result<ProviderStream> {
            let (tx, rx) = mpsc::channel(1);
            let cancellation = context.cancellation.child_token();
            let producer_cancellation = cancellation.clone();
            let started = self.started.clone();
            let finished = self.finished.clone();
            let call = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let producer = tokio::spawn(async move {
                if call == 0 {
                    started.notify_one();
                    producer_cancellation.cancelled().await;
                    finished.store(true, AtomicOrdering::SeqCst);
                    drop(tx);
                } else {
                    let accumulator = crate::ai::stream::MessageAccumulator::new("mock", "mock", "mock");
                    let _ = tx
                        .send(crate::ai::stream::StreamEvent::Done {
                            message: accumulator.build(),
                        })
                        .await;
                }
            });
            Ok(ProviderStream::new(rx, producer, cancellation))
        }
    }

    async fn next_event(events: &mut SessionControllerEventReceiver) -> SessionControllerEvent {
        timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("controller event should arrive")
            .expect("controller event channel should remain open")
    }

    async fn shutdown(mut connection: SessionControllerConnection) {
        connection.handle.shutdown().await.unwrap();
        connection.task.join().await.unwrap();
        while connection.events.recv().await.is_some() {}
    }

    #[tokio::test]
    async fn spawn_is_idle_and_shutdown_settles_task() {
        let mut connection = SessionController::spawn(mock_session(MockProvider::text("ok")));
        assert!(matches!(
            next_event(&mut connection.events).await,
            SessionControllerEvent::StateChanged(ControllerActivity::Idle)
        ));
        assert_eq!(
            connection.handle.snapshot().await.unwrap().activity,
            ControllerActivity::Idle
        );
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn text_prompt_forwards_one_terminal_event_and_returns_idle() {
        let mut connection = SessionController::spawn(mock_session(MockProvider::text("hello")));
        let _ = next_event(&mut connection.events).await;
        let submission = connection.handle.submit_prompt("hi").await.unwrap();
        let (request_id, run_id) = match submission {
            PromptSubmission::Accepted { request_id, run_id, .. } => (request_id, run_id),
            other => panic!("unexpected submission: {other:?}"),
        };
        let mut agent_events = Vec::new();
        loop {
            if let SessionControllerEvent::Agent {
                request_id: seen,
                event,
            } = next_event(&mut connection.events).await
            {
                assert_eq!(seen, request_id);
                agent_events.push(event);
                if matches!(agent_events.last(), Some(AgentEvent::RunFinished { .. })) {
                    break;
                }
            }
        }
        assert!(
            agent_events
                .iter()
                .any(|event| matches!(event, AgentEvent::RunStarted { run_id: seen } if *seen == run_id))
        );
        assert_eq!(agent_events.iter().filter(|event| is_terminal(event)).count(), 1);
        assert!(matches!(
            next_event(&mut connection.events).await,
            SessionControllerEvent::StateChanged(ControllerActivity::Idle)
        ));
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn unknown_slash_is_rejected_without_agent_event() {
        let mut connection = SessionController::spawn(mock_session(MockProvider::text("must not run")));
        let _ = next_event(&mut connection.events).await;
        let result = connection.handle.submit_prompt("/not-a-command").await.unwrap();
        assert!(matches!(
            result,
            PromptSubmission::Rejected {
                reason: PromptRejection::UnknownSlash { .. },
                ..
            }
        ));
        assert!(matches!(
            next_event(&mut connection.events).await,
            SessionControllerEvent::PromptRejected { .. }
        ));
        assert_eq!(
            connection.handle.snapshot().await.unwrap().activity,
            ControllerActivity::Idle
        );
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn failure_does_not_stop_controller() {
        let provider = MockProvider::new(vec![MockStep::Error("first".into()), MockStep::Text("second".into())]);
        let mut connection = SessionController::spawn(mock_session(provider));
        let _ = next_event(&mut connection.events).await;
        connection.handle.submit_prompt("one").await.unwrap();
        let mut saw_terminal = false;
        while !saw_terminal {
            if let SessionControllerEvent::Agent { event, .. } = next_event(&mut connection.events).await {
                saw_terminal = is_terminal(&event);
            }
        }
        assert!(matches!(
            next_event(&mut connection.events).await,
            SessionControllerEvent::StateChanged(ControllerActivity::Idle)
        ));
        assert!(matches!(
            connection.handle.submit_prompt("two").await.unwrap(),
            PromptSubmission::Accepted { .. }
        ));
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn tool_prompt_preserves_tool_events_and_terminal_order() {
        let provider = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "call-1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut connection = SessionController::spawn(session(Box::new(provider), vec![Box::new(EchoTool)]));
        let _ = next_event(&mut connection.events).await;
        let accepted = connection.handle.submit_prompt("use echo").await.unwrap();
        let request_id = match accepted {
            PromptSubmission::Accepted { request_id, .. } => request_id,
            other => panic!("unexpected submission: {other:?}"),
        };
        let mut names = Vec::new();
        loop {
            if let SessionControllerEvent::Agent {
                request_id: seen,
                event,
            } = next_event(&mut connection.events).await
            {
                assert_eq!(seen, request_id);
                names.push(match &event {
                    AgentEvent::ToolStarted { .. } => "started",
                    AgentEvent::ToolFinished { .. } => "finished",
                    AgentEvent::RunFinished { .. } => "terminal",
                    _ => "other",
                });
                if matches!(event, AgentEvent::RunFinished { .. }) {
                    break;
                }
            }
        }
        let started_index = names.iter().position(|name| *name == "started").unwrap();
        let finished_index = names.iter().position(|name| *name == "finished").unwrap();
        let terminal_index = names.iter().position(|name| *name == "terminal").unwrap();
        assert!(started_index < finished_index && finished_index < terminal_index);
        assert_eq!(names.iter().filter(|name| **name == "terminal").count(), 1);
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn cancellation_settles_stalled_provider_and_can_continue() {
        let started = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let provider = StalledProvider {
            started: started.clone(),
            finished: finished.clone(),
            calls: Arc::new(AtomicUsize::new(0)),
        };
        let mut connection = SessionController::spawn(session(Box::new(provider), vec![]));
        let _ = next_event(&mut connection.events).await;
        let accepted = connection.handle.submit_prompt("stall").await.unwrap();
        let (request_id, run_id) = match accepted {
            PromptSubmission::Accepted { request_id, run_id, .. } => (request_id, run_id),
            other => panic!("unexpected submission: {other:?}"),
        };
        timeout(Duration::from_secs(2), started.notified()).await.unwrap();
        assert_eq!(
            connection.handle.cancel_current().await.unwrap(),
            CancelResult::CancellationRequested { request_id, run_id }
        );
        let mut saw_abort = false;
        while !saw_abort {
            if let SessionControllerEvent::Agent { event, .. } = next_event(&mut connection.events).await {
                saw_abort = matches!(event, AgentEvent::RunAborted { .. });
            }
        }
        assert!(matches!(
            next_event(&mut connection.events).await,
            SessionControllerEvent::StateChanged(ControllerActivity::Idle)
        ));
        assert!(finished.load(AtomicOrdering::SeqCst));
        // A cancelled controller remains reusable; MockProvider is not needed
        // for this assertion because the provider's terminal settlement is the
        // lifecycle seam under test.
        assert_eq!(
            connection.handle.cancel_current().await.unwrap(),
            CancelResult::NothingRunning
        );
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn busy_mutations_do_not_change_snapshot_or_queue_work() {
        let started = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let mut connection = SessionController::spawn(session(
            Box::new(StalledProvider {
                started: started.clone(),
                finished,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vec![],
        ));
        let _ = next_event(&mut connection.events).await;
        let accepted = connection.handle.submit_prompt("busy").await.unwrap();
        assert!(matches!(accepted, PromptSubmission::Accepted { .. }));
        timeout(Duration::from_secs(2), started.notified()).await.unwrap();
        let before = connection.handle.snapshot().await.unwrap();
        assert!(matches!(before.activity, ControllerActivity::Running { .. }));
        assert!(matches!(
            connection.handle.submit_prompt("queued?").await,
            Err(ControllerRequestError::Busy(_))
        ));
        assert!(matches!(
            connection.handle.switch_model("other").await,
            Err(ControllerRequestError::Busy(_))
        ));
        assert!(matches!(
            connection.handle.add_context(PathBuf::from("x"), "x".into()).await,
            Err(ControllerRequestError::Busy(_))
        ));
        let during = connection.handle.snapshot().await.unwrap();
        assert_eq!(during.current_run_id, before.current_run_id);
        let _ = connection.handle.cancel_current().await.unwrap();
        assert!(matches!(
            connection.handle.submit_prompt("after cancel").await.unwrap(),
            PromptSubmission::Accepted { .. }
        ));
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn snapshot_tracks_context_and_running_identity() {
        let started = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let mut connection = SessionController::spawn(session(
            Box::new(StalledProvider {
                started: started.clone(),
                finished,
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vec![],
        ));
        let _ = next_event(&mut connection.events).await;
        let path = PathBuf::from("context.md");
        connection
            .handle
            .add_context(path.clone(), "context".into())
            .await
            .unwrap();
        let idle = connection.handle.snapshot().await.unwrap();
        assert_eq!(idle.activity, ControllerActivity::Idle);
        assert_eq!(idle.context_files, vec![path]);
        let accepted = connection.handle.submit_prompt("hello").await.unwrap();
        let run_id = match accepted {
            PromptSubmission::Accepted { run_id, .. } => run_id,
            other => panic!("unexpected submission: {other:?}"),
        };
        timeout(Duration::from_secs(2), started.notified()).await.unwrap();
        let running = connection.handle.snapshot().await.unwrap();
        assert_eq!(running.current_run_id, Some(run_id));
        assert!(matches!(running.activity, ControllerActivity::Running { .. }));
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn reserved_prompt_is_rejected_without_a_run() {
        let mut connection = SessionController::spawn(mock_session(MockProvider::text("must not run")));
        let _ = next_event(&mut connection.events).await;
        let result = connection.handle.submit_prompt("/help").await.unwrap();
        assert!(matches!(
            result,
            PromptSubmission::Rejected {
                reason: PromptRejection::ReservedCommand { .. },
                ..
            }
        ));
        assert_eq!(connection.handle.snapshot().await.unwrap().current_run_id, None);
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn known_template_is_expanded_inside_controller_only() {
        let directory = tempfile::tempdir().unwrap();
        let template = directory.path().join("review.md");
        std::fs::write(&template, "Review $1").unwrap();
        let provider = MockProvider::text("done");
        let captured = provider.captured_requests_arc();
        let prompt_session = PromptSession::new(
            Box::new(provider),
            Model {
                id: "mock",
                api: "mock",
            },
            vec![],
            PathBuf::from("/tmp"),
            PathBuf::from("/tmp/.pi/agent"),
            vec![template],
            vec![],
            false,
            None,
            vec![],
        );
        let mut connection = SessionController::spawn(prompt_session);
        let _ = next_event(&mut connection.events).await;
        let accepted = connection.handle.submit_prompt("/review src/").await.unwrap();
        assert!(matches!(accepted, PromptSubmission::Accepted { original, .. } if original == "/review src/"));
        let mut terminal = false;
        while !terminal {
            if let SessionControllerEvent::Agent { event, .. } = next_event(&mut connection.events).await {
                terminal = is_terminal(&event);
            }
        }
        let _ = next_event(&mut connection.events).await;
        let messages = captured.lock().unwrap().clone();
        assert!(matches!(
            &messages[0][0],
            crate::ai::types::AgentMessage::User(user)
                if matches!(&user.content, crate::ai::types::MessageContent::Text(text) if text == "Review src/")
        ));
        drop(messages);
        shutdown(connection).await;
    }

    #[tokio::test]
    async fn shutdown_during_run_cancels_and_settles_before_join() {
        let started = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let mut connection = SessionController::spawn(session(
            Box::new(StalledProvider {
                started: started.clone(),
                finished: finished.clone(),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vec![],
        ));
        let _ = next_event(&mut connection.events).await;
        connection.handle.submit_prompt("shutdown me").await.unwrap();
        timeout(Duration::from_secs(2), started.notified()).await.unwrap();
        connection.handle.shutdown().await.unwrap();
        assert!(finished.load(AtomicOrdering::SeqCst));
        connection.task.join().await.unwrap();
        let mut saw_abort = false;
        let mut saw_stopped = false;
        while let Some(event) = connection.events.recv().await {
            saw_abort |= matches!(
                event,
                SessionControllerEvent::Agent {
                    event: AgentEvent::RunAborted { .. },
                    ..
                }
            );
            saw_stopped |= matches!(event, SessionControllerEvent::Stopped);
        }
        assert!(saw_abort && saw_stopped);
        assert_eq!(connection.handle.shutdown().await, Err(ControllerRequestError::Stopped));
    }

    #[tokio::test]
    async fn dropped_event_receiver_does_not_panic_or_detach_run() {
        let connection = SessionController::spawn(mock_session(MockProvider::text("ok")));
        let SessionControllerConnection { handle, events, task } = connection;
        drop(events);
        assert!(matches!(
            handle.submit_prompt("still settles").await.unwrap(),
            PromptSubmission::Accepted { .. }
        ));
        handle.shutdown().await.unwrap();
        task.join().await.unwrap();
    }

    #[tokio::test]
    async fn dropped_request_sender_auto_shuts_down_controller() {
        let connection = SessionController::spawn(mock_session(MockProvider::text("ok")));
        let SessionControllerConnection { handle, events, task } = connection;
        drop(handle);
        drop(events);
        timeout(Duration::from_secs(2), task.join()).await.unwrap().unwrap();
    }
}
