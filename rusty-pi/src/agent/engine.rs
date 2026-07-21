//! Agent — the core loop that drives LLM ↔ tool interactions.

use crate::agent::events::{AgentEvent, ProviderError, RunId};
use crate::agent::session::Session;
use crate::agent::types::{AgentTool, AgentToolResult, ToolExecutionContext, ToolOutputEvent};
use crate::ai::providers::{Model, ProviderApi, ProviderRequestContext};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{
    AgentMessage, AgentToolCall, AssistantContent, Content, MessageContent, StopReason, TextOrImageContent, Tool,
    ToolResultMessage, UserMessage,
};
use anyhow::{Context, Result};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Configuration for the agent.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub system_prompt: String,
    pub max_tool_rounds: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            system_prompt: String::new(),
            max_tool_rounds: 25,
        }
    }
}

/// Shared cancellation token for signalling Ctrl+C / cancellation.
/// Each agent run creates a child token; cancelling the parent cancels all runs.
pub type AbortFlag = CancellationToken;

/// The result of one tool execution before it is projected into the session.
/// The original result remains available for frontend events and loop control;
/// the session message is a separate, wire-oriented projection.
struct ExecutedToolOutcome {
    result: AgentToolResult,
    session_message: AgentMessage,
}

/// The agent that orchestrates the LLM → tool → LLM loop.
pub struct Agent {
    session: Session,
    tools: Vec<Box<dyn AgentTool>>,
    provider: Box<dyn ProviderApi>,
    model: Model,
    config: AgentConfig,
    /// Shared cancellation token: cancelling this aborts the current run.
    abort: AbortFlag,
    /// Optional event sender for unified event stream.
    event_tx: Option<mpsc::Sender<AgentEvent>>,
    /// Monotonic counter for generating RunIds.
    run_counter: u64,
}

impl Agent {
    pub fn new(provider: Box<dyn ProviderApi>, model: Model) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            session: Session::in_memory(cwd),
            tools: Vec::new(),
            provider,
            model,
            config: AgentConfig::default(),
            abort: CancellationToken::new(),
            event_tx: None,
            run_counter: 0,
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Signal the agent to abort the current round.
    /// The next tick of the stream loop will notice and return `StopReason::Aborted`.
    pub fn abort(&self) {
        self.abort.cancel();
    }

    /// Switch the model used by this agent at runtime.
    /// Provider stays the same; only the model ID changes.
    pub fn switch_model(&mut self, model: Model) {
        self.model = model;
    }

    /// Return the current model.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// List models available through this agent's provider.
    pub fn list_models(&self) -> Vec<&Model> {
        self.provider.list_models()
    }

    /// Replace the cancellation token (used to share a token between REPL and agent).
    pub fn set_abort_flag(&mut self, token: AbortFlag) {
        self.abort = token;
    }

    /// Get a reference to the cancellation token (for sharing with the REPL).
    pub fn abort_flag(&self) -> AbortFlag {
        self.abort.clone()
    }

    /// Set the event sender for this agent. All events will be sent through this channel.
    pub fn set_event_sender(&mut self, tx: mpsc::Sender<AgentEvent>) {
        self.event_tx = Some(tx);
    }

    /// Get a receiver for agent events. Creates a new channel if none exists.
    pub fn event_receiver(&mut self) -> mpsc::Receiver<AgentEvent> {
        let (tx, rx) = mpsc::channel(256);
        self.event_tx = Some(tx);
        rx
    }

    /// Get a reference to the event sender (for dropping to close channel).
    pub fn event_sender_ref(&self) -> Option<&mpsc::Sender<AgentEvent>> {
        self.event_tx.as_ref()
    }

    /// Send an event if a sender is configured.
    async fn emit(&self, event: AgentEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event).await;
        }
    }

    pub fn add_tool(&mut self, tool: Box<dyn AgentTool>) {
        self.tools.push(tool);
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt;
    }

    /// Return the current derived system prompt.
    pub fn system_prompt(&self) -> &str {
        &self.config.system_prompt
    }

    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.session.messages().await
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Get a reference to the provider (for test assertions).
    pub fn provider(&self) -> &dyn ProviderApi {
        self.provider.as_ref()
    }

    /// Replace the session backing this agent (e.g., with a JSONL-persisted session).
    pub fn set_session(&mut self, session: Session) {
        self.session = session;
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    async fn finish_aborted(&mut self, run_id: RunId) {
        let msg = crate::ai::types::AssistantMessage {
            content: vec![],
            api: self.model.api.to_string(),
            provider: self.model.id.to_string(),
            model: self.model.id.to_string(),
            usage: None,
            stop_reason: StopReason::Aborted,
            error_message: Some("Aborted by user".into()),
            timestamp: Self::now_ms(),
        };
        let _ = self.session.append_message(AgentMessage::Assistant(msg)).await;
        self.emit(AgentEvent::RunAborted { run_id }).await;
    }

    fn tool_refs(&self) -> Vec<&dyn Tool> {
        self.tools.iter().map(|t| t.as_ref() as &dyn Tool).collect()
    }

    /// Run the agent with a prompt. All events are emitted through the event channel.
    ///
    /// Every `RunStarted` is guaranteed to produce exactly one terminal event
    /// (`RunFinished`, `RunAborted`, or `RunFailed`). The implementation uses
    /// a centralized exit path via `finish_run` to enforce this invariant.
    pub async fn run(&mut self, prompt: &str) -> Result<()> {
        let now = Self::now_ms();

        // Generate a unique RunId for this run
        self.run_counter += 1;
        let run_id = RunId::new(self.run_counter);

        let user_msg = AgentMessage::User(UserMessage {
            content: MessageContent::Text(prompt.to_string()),
            timestamp: now,
        });
        if let Err(e) = self.session.append_message(user_msg).await {
            let err = format!("Session error: {}", e);
            self.emit(AgentEvent::RunStarted { run_id }).await;
            self.emit(AgentEvent::RunFailed {
                run_id,
                error: crate::agent::events::AgentRunError {
                    phase: crate::agent::events::AgentRunPhase::Session,
                    message: err.clone(),
                },
            })
            .await;
            return Err(anyhow::anyhow!(err));
        }

        // Create a child token for this run. If the parent is already cancelled,
        // this child will be immediately cancelled too.
        let run_token = self.abort.child_token();

        // Emit RunStarted
        self.emit(AgentEvent::RunStarted { run_id }).await;

        // Run the inner loop, capturing the result
        let result = self.run_inner(run_id, run_token).await;

        // Centralized terminal event emission
        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                let err_msg = format!("{}", e);
                // Don't emit RunFailed if the run was already aborted
                if !self.abort.is_cancelled() {
                    self.emit(AgentEvent::RunFailed {
                        run_id,
                        error: crate::agent::events::AgentRunError {
                            phase: crate::agent::events::AgentRunPhase::AgentLoop,
                            message: err_msg.clone(),
                        },
                    })
                    .await;
                }
                Err(e)
            }
        }
    }

    /// Inner run loop — returns Ok(()) on any terminal event, Err on internal failure.
    /// Terminal events (RunFinished, RunAborted, RunFailed) are emitted here.
    async fn run_inner(&mut self, run_id: RunId, run_token: CancellationToken) -> Result<()> {
        for round in 0..=self.config.max_tool_rounds {
            let tool_refs = self.tool_refs();
            let current_messages = self.session.messages().await;

            // System prompt is passed separately via the provider API
            let system_prompt = if self.config.system_prompt.is_empty() {
                None
            } else {
                Some(self.config.system_prompt.as_str())
            };

            let request_context = ProviderRequestContext::new(run_token.clone());
            let provider_result = tokio::select! {
                biased;
                _ = run_token.cancelled() => None,
                result = self.provider.stream(
                    &self.model,
                    &current_messages,
                    &tool_refs,
                    system_prompt,
                    request_context,
                ) => Some(result),
            };
            let Some(provider_result) = provider_result else {
                self.finish_aborted(run_id).await;
                return Ok(());
            };
            let mut stream = match provider_result {
                Ok(s) => s,
                Err(e) => {
                    self.emit(AgentEvent::RunFailed {
                        run_id,
                        error: crate::agent::events::AgentRunError {
                            phase: crate::agent::events::AgentRunPhase::ProviderStart,
                            message: format!("LLM call failed at round {}: {}", round, e),
                        },
                    })
                    .await;
                    return Err(anyhow::anyhow!("LLM call failed at round {}: {}", round, e));
                }
            };

            // Accumulate stream events into an AssistantMessage
            let mut acc = MessageAccumulator::new(self.model.api, self.model.id, self.model.id);

            // Check for abort BEFORE starting the stream loop
            if run_token.is_cancelled() {
                stream.cancel_and_shutdown().await;
                self.finish_aborted(run_id).await;
                return Ok(());
            }

            while let Some(event) = tokio::select! {
                biased;
                _ = run_token.cancelled() => {
                    stream.cancel_and_shutdown().await;
                    self.finish_aborted(run_id).await;
                    return Ok(());
                }
                event = stream.recv() => event,
            } {
                if run_token.is_cancelled() {
                    stream.cancel_and_shutdown().await;
                    self.finish_aborted(run_id).await;
                    return Ok(());
                }

                // Emit through event channel for text deltas
                if let StreamEvent::TextDelta { ref delta } = event {
                    self.emit(AgentEvent::TextDelta {
                        run_id,
                        text: delta.clone(),
                    })
                    .await;
                }

                let is_terminal = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
                acc.push(event);
                if is_terminal {
                    break;
                }
            }

            // A terminal event means the producer should already be done. If
            // the receiver closed naturally, awaiting it here still proves
            // that no producer task was detached.
            stream.shutdown().await;

            // Check for abort AFTER the stream loop (user hit Ctrl+C during streaming)
            if run_token.is_cancelled() {
                self.finish_aborted(run_id).await;
                return Ok(());
            }

            let response = acc.build();

            // Check for provider error
            if response.stop_reason == StopReason::Error {
                if let Some(err_msg) = &response.error_message {
                    self.emit(AgentEvent::ProviderError {
                        run_id,
                        error: ProviderError {
                            reason: StopReason::Error,
                            message: err_msg.clone(),
                        },
                    })
                    .await;
                }
            }

            // Extract tool calls as AgentToolCall structs
            let tool_calls: Vec<AgentToolCall> = response
                .content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::ToolCall { id, name, arguments } => Some(AgentToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    }),
                    _ => None,
                })
                .collect();

            let has_tool_calls = !tool_calls.is_empty();
            let stop_reason = if has_tool_calls && response.stop_reason == StopReason::Stop {
                // Only override Stop → ToolUse. Length/Error/Aborted responses should not execute tools
                // even when they contain tool call blocks.
                StopReason::ToolUse
            } else {
                response.stop_reason
            };

            let response = crate::ai::types::AssistantMessage {
                stop_reason,
                ..response
            };

            if let Err(e) = self
                .session
                .append_message(AgentMessage::Assistant(response.clone()))
                .await
            {
                self.emit(AgentEvent::RunFailed {
                    run_id,
                    error: crate::agent::events::AgentRunError {
                        phase: crate::agent::events::AgentRunPhase::Session,
                        message: format!("Session append failed: {}", e),
                    },
                })
                .await;
                return Err(anyhow::anyhow!("Session append failed: {}", e));
            }

            match response.stop_reason {
                StopReason::Stop | StopReason::Length | StopReason::Error | StopReason::Aborted => {
                    self.emit(AgentEvent::RunFinished {
                        run_id,
                        stop_reason: response.stop_reason,
                    })
                    .await;
                    return Ok(());
                }
                StopReason::ToolUse => {
                    if round >= self.config.max_tool_rounds {
                        self.emit(AgentEvent::RunFailed {
                            run_id,
                            error: crate::agent::events::AgentRunError {
                                phase: crate::agent::events::AgentRunPhase::AgentLoop,
                                message: format!("Exceeded maximum tool call rounds ({})", self.config.max_tool_rounds),
                            },
                        })
                        .await;
                        anyhow::bail!("Exceeded maximum tool call rounds ({})", self.config.max_tool_rounds);
                    }

                    let mut any_terminate = false;
                    for call in &tool_calls {
                        // A cancellation that arrives during an earlier tool
                        // must not start another tool from this batch.
                        if run_token.is_cancelled() {
                            break;
                        }
                        // Emit ToolStarted
                        self.emit(AgentEvent::ToolStarted {
                            run_id,
                            tool_call_id: call.id.clone(),
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        })
                        .await;

                        let outcome = match self
                            .execute_tool(run_id, &call.id, &call.name, call.arguments.clone(), &run_token)
                            .await
                        {
                            Ok(o) => o,
                            Err(e) => {
                                self.emit(AgentEvent::RunFailed {
                                    run_id,
                                    error: crate::agent::events::AgentRunError {
                                        phase: crate::agent::events::AgentRunPhase::ToolExecution,
                                        message: format!("Tool '{}' failed: {}", call.name, e),
                                    },
                                })
                                .await;
                                return Err(anyhow::anyhow!("Tool '{}' execution failed: {}", call.name, e));
                            }
                        };

                        // Emit ToolFinished
                        self.emit(AgentEvent::ToolFinished {
                            run_id,
                            tool_call_id: call.id.clone(),
                            name: call.name.clone(),
                            result: outcome.result.clone(),
                        })
                        .await;

                        if let Err(e) = self.session.append_message(outcome.session_message).await {
                            self.emit(AgentEvent::RunFailed {
                                run_id,
                                error: crate::agent::events::AgentRunError {
                                    phase: crate::agent::events::AgentRunPhase::Session,
                                    message: format!("Session append failed: {}", e),
                                },
                            })
                            .await;
                            return Err(anyhow::anyhow!("Session append failed: {}", e));
                        }
                        if outcome.result.terminate {
                            any_terminate = true;
                        }
                    }
                    if run_token.is_cancelled() {
                        self.finish_aborted(run_id).await;
                        return Ok(());
                    }
                    if any_terminate {
                        self.emit(AgentEvent::RunFinished {
                            run_id,
                            stop_reason: StopReason::Stop,
                        })
                        .await;
                        return Ok(());
                    }
                }
            }
        }

        self.emit(AgentEvent::RunFailed {
            run_id,
            error: crate::agent::events::AgentRunError {
                phase: crate::agent::events::AgentRunPhase::AgentLoop,
                message: "Agent loop exited without producing a final response".into(),
            },
        })
        .await;
        anyhow::bail!("Agent loop exited without producing a final response")
    }

    /// Execute a tool and retain the complete result before projecting it into
    /// the session message.
    ///
    /// Creates a per-execution [`ToolExecutionContext`] with:
    /// - A child token of `run_token` for direct cancellation.
    /// - An independent output channel for streaming chunks.
    ///
    /// ToolOutput events are forwarded to the agent's event channel.
    async fn execute_tool(
        &self,
        run_id: RunId,
        tool_call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
        run_token: &CancellationToken,
    ) -> Result<ExecutedToolOutcome> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == tool_name)
            .with_context(|| format!("Tool '{}' not found", tool_name))?;

        let _start_ms = Self::now_ms();

        // Create a per-execution context with its own cancellation token and output channel.
        let tool_token = run_token.child_token();
        let (output_tx, mut output_rx) = tokio::sync::mpsc::channel::<ToolOutputEvent>(256);
        let context = ToolExecutionContext {
            output_tx,
            cancellation: tool_token,
        };

        // Spawn a task to forward ToolOutput events to the agent's event channel.
        let event_tx_clone = self.event_tx.clone();
        let fwd_tool_call_id = tool_call_id.to_string();
        let fwd_run_id = run_id;
        let forwarder = tokio::spawn(async move {
            while let Some(evt) = output_rx.recv().await {
                if let Some(ref tx) = event_tx_clone {
                    let _ = tx
                        .send(AgentEvent::ToolOutput {
                            run_id: fwd_run_id,
                            tool_call_id: fwd_tool_call_id.clone(),
                            stream: evt.stream,
                            chunk: evt.chunk,
                        })
                        .await;
                }
            }
        });

        let result = tool
            .execute(tool_call_id, args, context)
            .await
            .with_context(|| format!("Tool '{}' execution failed", tool_name))?;

        // Wait for the forwarder to finish draining output events.
        let _ = forwarder.await;

        let now = Self::now_ms();

        let content: Vec<_> = result
            .content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(TextOrImageContent::Text { text: text.clone() }),
                Content::Image { data, mime_type } => Some(TextOrImageContent::Image {
                    data: data.clone(),
                    mime_type: mime_type.clone(),
                }),
                _ => None,
            })
            .collect();

        // Build structured details from AgentToolResult fields
        let details = {
            let mut d = match &result.details {
                serde_json::Value::Object(m) => m.clone(),
                _ => serde_json::Map::new(),
            };
            if let Some(code) = result.exit_code {
                d.insert("exit_code".into(), serde_json::json!(code));
            }
            if result.timed_out {
                d.insert("timed_out".into(), serde_json::json!(true));
            }
            if result.aborted {
                d.insert("aborted".into(), serde_json::json!(true));
            }
            serde_json::Value::Object(d)
        };

        let is_error = result.is_error;
        Ok(ExecutedToolOutcome {
            result,
            session_message: AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: tool_call_id.to_string(),
                tool_name: tool_name.to_string(),
                content,
                details: Some(details),
                is_error,
                timestamp: now,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::AgentToolResult;
    use crate::ai::mock::{MockProvider, MockStep};
    use crate::ai::providers::{Model, ProviderStream};
    use crate::ai::types::AssistantMessage;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, RwLock};
    use tokio::sync::Notify;

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            })
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
            params: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: format!("echo: {}", text),
                }],
                ..Default::default()
            })
        }
    }

    fn make_model() -> Model {
        Model {
            id: "mock",
            api: "mock",
        }
    }

    struct StalledProvider {
        started: Arc<Notify>,
        finished: Arc<AtomicBool>,
    }

    #[async_trait]
    impl ProviderApi for StalledProvider {
        async fn stream(
            &self,
            _model: &Model,
            _messages: &[AgentMessage],
            _tools: &[&dyn Tool],
            _system_prompt: Option<&str>,
            context: ProviderRequestContext,
        ) -> anyhow::Result<ProviderStream> {
            let (tx, rx) = mpsc::channel(1);
            let started = self.started.clone();
            let finished = self.finished.clone();
            let cancellation = context.cancellation.child_token();
            let producer_cancellation = cancellation.clone();
            let producer = tokio::spawn(async move {
                started.notify_one();
                producer_cancellation.cancelled().await;
                finished.store(true, Ordering::SeqCst);
                drop(tx);
            });
            Ok(ProviderStream::new(rx, producer, cancellation))
        }
    }

    #[tokio::test]
    async fn agent_cancellation_interrupts_stalled_provider_receive() {
        let started = Arc::new(Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let provider = StalledProvider {
            started: started.clone(),
            finished: finished.clone(),
        };
        let mut agent = Agent::new(Box::new(provider), make_model());
        let cancellation = agent.abort_flag();
        let mut events = agent.event_receiver();
        let run = agent.run("stall");
        tokio::pin!(run);

        tokio::select! {
            result = &mut run => panic!("agent completed before cancellation: {:?}", result),
            result = tokio::time::timeout(std::time::Duration::from_secs(2), started.notified()) => {
                result.expect("provider request should start");
            }
        }
        cancellation.cancel();

        tokio::time::timeout(std::time::Duration::from_secs(2), &mut run)
            .await
            .expect("agent should return promptly after provider cancellation")
            .expect("cancellation is not a provider error");
        assert!(finished.load(Ordering::SeqCst), "producer task must be awaited");

        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        assert!(
            received
                .iter()
                .any(|event| matches!(event, AgentEvent::RunAborted { .. }))
        );
        assert!(
            !received
                .iter()
                .any(|event| matches!(event, AgentEvent::ProviderError { .. }))
        );
        assert!(!received.iter().any(|event| {
            matches!(
                event,
                AgentEvent::TextDelta { .. }
                    | AgentEvent::ThinkingDelta { .. }
                    | AgentEvent::ToolStarted { .. }
                    | AgentEvent::ToolFinished { .. }
                    | AgentEvent::RunFinished { .. }
            )
        }));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            events.try_recv().is_err(),
            "cancelled provider must not emit late events"
        );
    }

    #[tokio::test]
    async fn agent_returns_text_response() {
        let mock = MockProvider::text("Hello from mock!");
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.run("Hi there").await.unwrap();
        let msgs = agent.messages().await;
        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Stop);
                assert!(a.content.iter().any(|c| matches!(c, AssistantContent::Text { .. })));
            }
            _ => panic!("Expected assistant message"),
        }
    }

    #[tokio::test]
    async fn agent_handles_tool_call_and_result() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "hello"}),
                stop_reason: None,
            },
            MockStep::Text("Done!".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Run echo").await.unwrap();
        let msgs = agent.messages().await;
        assert!(msgs.len() >= 4);
        match &msgs[msgs.len() - 2] {
            AgentMessage::ToolResult(tr) => assert_eq!(tr.tool_name, "echo"),
            _ => panic!("Expected tool result"),
        }
        match msgs.last().unwrap() {
            AgentMessage::Assistant(a) => assert_eq!(a.stop_reason, StopReason::Stop),
            _ => panic!("Expected assistant"),
        }
    }

    #[tokio::test]
    async fn agent_reports_provider_error() {
        let mock = MockProvider::new(vec![MockStep::Error("API error".into())]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.run("Trigger error").await.unwrap();
        let msgs = agent.messages().await;
        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Error);
                assert_eq!(a.error_message.as_deref(), Some("API error"));
            }
            _ => panic!("Expected assistant"),
        }
    }

    #[tokio::test]
    async fn agent_with_bash_tool() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_bash".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo bash-works"}),
                stop_reason: None,
            },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        let shared_cwd = Arc::new(RwLock::new(std::env::current_dir().unwrap()));
        agent.add_tool(Box::new(crate::coding_agent::tools::bash::BashTool::new(shared_cwd)));
        agent.run("Run bash").await.unwrap();
        let msgs = agent.messages().await;
        assert!(msgs.len() >= 4);
        match &msgs[msgs.len() - 2] {
            AgentMessage::ToolResult(tr) => assert_eq!(tr.tool_name, "bash"),
            _ => panic!("Expected tool result"),
        }
        match msgs.last().unwrap() {
            AgentMessage::Assistant(a) => assert_eq!(a.stop_reason, StopReason::Stop),
            _ => panic!("Expected assistant"),
        }
    }

    #[tokio::test]
    async fn agent_multiple_rounds() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "first"}),
                stop_reason: None,
            },
            MockStep::ToolCall {
                id: "tc_2".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "second"}),
                stop_reason: None,
            },
            MockStep::Text("All done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Do two things").await.unwrap();
        assert_eq!(
            agent
                .messages()
                .await
                .iter()
                .filter(|m| matches!(m, AgentMessage::ToolResult(_)))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn agent_does_not_execute_tool_calls_from_length_truncated_response() {
        // When the provider returns tool calls with StopReason::Length,
        // the agent should NOT execute those tool calls.
        let mock = MockProvider::new(vec![MockStep::ToolCall {
            id: "tc_1".into(),
            name: "echo".into(),
            arguments: serde_json::json!({"text": "should-not-run"}),
            stop_reason: Some(StopReason::Length),
        }]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Run echo").await.unwrap();
        let msgs = agent.messages().await;
        // Only 2 messages: user + assistant (no tool result)
        assert_eq!(msgs.len(), 2, "Expected no tool result for length-truncated response");
        match msgs.last().unwrap() {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Length);
            }
            _ => panic!("Expected assistant message"),
        }
    }

    #[tokio::test]
    async fn agent_terminate_flag_stops_loop() {
        // A tool that signals termination should stop the agent loop after its round.
        let mock = MockProvider::new(vec![MockStep::ToolCall {
            id: "tc_1".into(),
            name: "terminator".into(),
            arguments: serde_json::json!({}),
            stop_reason: None,
        }]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(TerminatorTool));
        agent.run("Terminate").await.unwrap();
        let msgs = agent.messages().await;
        // user + assistant + tool_result = 3 messages (no second round)
        assert_eq!(msgs.len(), 3, "Should stop after terminated tool");
    }

    struct TerminatorTool;

    impl Tool for TerminatorTool {
        fn name(&self) -> &str {
            "terminator"
        }
        fn description(&self) -> &str {
            "Terminates the loop"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
    }

    #[async_trait]
    impl AgentTool for TerminatorTool {
        fn label(&self) -> &str {
            "terminator"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "done".into() }],
                terminate: true,
                ..Default::default()
            })
        }
    }

    struct FidelityTool;

    impl Tool for FidelityTool {
        fn name(&self) -> &str {
            "fidelity"
        }

        fn description(&self) -> &str {
            "returns a complete structured result"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    #[async_trait]
    impl AgentTool for FidelityTool {
        fn label(&self) -> &str {
            "fidelity"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            context
                .output_tx
                .send(ToolOutputEvent {
                    stream: crate::agent::events::ToolOutputStream::Stdout,
                    chunk: "stdout chunk".into(),
                })
                .await
                .unwrap();
            context
                .output_tx
                .send(ToolOutputEvent {
                    stream: crate::agent::events::ToolOutputStream::Stderr,
                    chunk: "stderr chunk".into(),
                })
                .await
                .unwrap();
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "complete tool output".into(),
                }],
                is_error: true,
                details: serde_json::json!({"structured": {"kind": "fidelity"}}),
                exit_code: Some(23),
                timed_out: true,
                aborted: true,
                terminate: true,
                added_tool_names: Some(vec!["future_tool".into()]),
            })
        }
    }

    #[tokio::test]
    async fn tool_finished_event_preserves_complete_tool_outcome() {
        let mock = MockProvider::new(vec![MockStep::ToolCall {
            id: "tc_fidelity".into(),
            name: "fidelity".into(),
            arguments: serde_json::json!({}),
            stop_reason: None,
        }]);
        let captured = mock.captured_requests_arc();
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(FidelityTool));
        let mut events = agent.event_receiver();

        agent.run("preserve this result").await.unwrap();

        let mut finished = Vec::new();
        let mut output_streams = Vec::new();
        while let Ok(event) = events.try_recv() {
            match event {
                AgentEvent::ToolFinished { result, .. } => finished.push(result),
                AgentEvent::ToolOutput { stream, chunk, .. } => output_streams.push((stream, chunk)),
                _ => {}
            }
        }
        assert_eq!(finished.len(), 1, "one tool call must produce one completion event");
        let result = &finished[0];
        assert_eq!(result.content.len(), 1);
        assert!(result.is_error);
        assert_eq!(result.details["structured"]["kind"], "fidelity");
        assert_eq!(result.exit_code, Some(23));
        assert!(result.timed_out);
        assert!(result.aborted);
        assert!(result.terminate);
        assert_eq!(
            result.added_tool_names.as_deref(),
            Some(["future_tool".to_string()].as_slice())
        );
        assert!(output_streams.iter().any(|(stream, chunk)| {
            *stream == crate::agent::events::ToolOutputStream::Stdout && chunk == "stdout chunk"
        }));
        assert!(output_streams.iter().any(|(stream, chunk)| {
            *stream == crate::agent::events::ToolOutputStream::Stderr && chunk == "stderr chunk"
        }));

        let messages = agent.messages().await;
        let session_result = messages
            .iter()
            .find_map(|message| match message {
                AgentMessage::ToolResult(result) => Some(result),
                _ => None,
            })
            .expect("tool result must be persisted");
        assert!(session_result.is_error);
        assert_eq!(
            session_result.details.as_ref().unwrap()["structured"]["kind"],
            "fidelity"
        );
        assert_eq!(session_result.details.as_ref().unwrap()["exit_code"], 23);
        assert_eq!(session_result.details.as_ref().unwrap()["timed_out"], true);
        assert_eq!(session_result.details.as_ref().unwrap()["aborted"], true);
        assert_eq!(
            captured.lock().unwrap().len(),
            1,
            "terminate must stop the next model round"
        );
    }

    struct CancellingBatchTool {
        cancellation: CancellationToken,
        calls: Arc<AtomicUsize>,
    }

    impl Tool for CancellingBatchTool {
        fn name(&self) -> &str {
            "cancel_batch"
        }

        fn description(&self) -> &str {
            "cancels after the first invocation"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
    }

    #[async_trait]
    impl AgentTool for CancellingBatchTool {
        fn label(&self) -> &str {
            "cancel_batch"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.cancellation.cancel();
            }
            Ok(AgentToolResult {
                content: vec![Content::Text {
                    text: "cancelled".into(),
                }],
                is_error: true,
                aborted: true,
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn cancellation_skips_tools_not_started_in_current_batch() {
        use crate::ai::mock::MultiToolCallProvider;

        let cancellation = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = MultiToolCallProvider::new(
            vec![
                MockStep::ToolCall {
                    id: "first".into(),
                    name: "cancel_batch".into(),
                    arguments: serde_json::json!({}),
                    stop_reason: None,
                },
                MockStep::ToolCall {
                    id: "second".into(),
                    name: "cancel_batch".into(),
                    arguments: serde_json::json!({}),
                    stop_reason: None,
                },
            ],
            "must not request another round",
        );
        let mut agent = Agent::new(Box::new(provider), make_model());
        agent.set_abort_flag(cancellation.clone());
        agent.add_tool(Box::new(CancellingBatchTool {
            cancellation,
            calls: calls.clone(),
        }));
        let mut events = agent.event_receiver();

        agent.run("cancel batch").await.unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut started = 0;
        let mut aborted = false;
        while let Ok(event) = events.try_recv() {
            match event {
                AgentEvent::ToolStarted { .. } => started += 1,
                AgentEvent::RunAborted { .. } => aborted = true,
                _ => {}
            }
        }
        assert_eq!(started, 1);
        assert!(aborted);
    }

    // ── Task 二: Complete tool call round-trip with second request verification ──

    #[tokio::test]
    async fn agent_complete_tool_call_round_trip_second_request_includes_history() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_roundtrip".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "round-trip"}),
                stop_reason: None,
            },
            MockStep::Text("Round trip complete".into()),
        ]);
        let captured = mock.captured_requests_arc();
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Do a round trip").await.unwrap();

        // The provider should have been called twice
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2, "Provider should have been called twice");

        // First request: just the user message
        let req1 = &requests[0];
        assert_eq!(req1.len(), 1);
        assert!(
            matches!(&req1[0], AgentMessage::User(u) if u.content == MessageContent::Text("Do a round trip".into()))
        );

        // Second request: user + assistant(tool_call) + tool_result
        let req2 = &requests[1];
        assert_eq!(
            req2.len(),
            3,
            "Second request should have user + assistant + tool_result"
        );

        // Verify message types
        assert!(matches!(&req2[0], AgentMessage::User(_)));
        assert!(matches!(&req2[1], AgentMessage::Assistant(a) if a.stop_reason == StopReason::ToolUse));
        assert!(matches!(&req2[2], AgentMessage::ToolResult(tr) if tr.tool_call_id == "tc_roundtrip"));

        // Verify assistant message has tool call
        if let AgentMessage::Assistant(a) = &req2[1] {
            let tc = a.content.iter().find_map(|c| match c {
                AssistantContent::ToolCall { id, name, arguments } => Some((id, name, arguments)),
                _ => None,
            });
            let (id, name, args) = tc.expect("Should have tool call in assistant message");
            assert_eq!(id, "tc_roundtrip");
            assert_eq!(name, "echo");
            assert_eq!(args["text"], "round-trip");
        }

        // Verify tool result message
        if let AgentMessage::ToolResult(tr) = &req2[2] {
            assert_eq!(tr.tool_call_id, "tc_roundtrip");
            assert_eq!(tr.tool_name, "echo");
            assert!(!tr.is_error);
            let text = tr.content.iter().find_map(|c| match c {
                TextOrImageContent::Text { text } => Some(text.as_str()),
                _ => None,
            });
            assert_eq!(text, Some("echo: round-trip"));
        }
    }

    // ── Task 三: Multiple tool calls in one response ──

    #[tokio::test]
    async fn agent_multiple_tool_calls_in_single_response() {
        use crate::ai::mock::MultiToolCallProvider;

        let multi = MultiToolCallProvider::new(
            vec![
                MockStep::ToolCall {
                    id: "tc_multi_a".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({"text": "alpha"}),
                    stop_reason: None,
                },
                MockStep::ToolCall {
                    id: "tc_multi_b".into(),
                    name: "echo".into(),
                    arguments: serde_json::json!({"text": "beta"}),
                    stop_reason: None,
                },
            ],
            "Both done",
        );
        let mut agent = Agent::new(Box::new(multi), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Two echoes").await.unwrap();

        let msgs = agent.messages().await;
        // user + assistant(2 tool calls) + tool_result(1) + tool_result(2) + assistant(final) = 5
        assert_eq!(
            msgs.len(),
            5,
            "Should have 5 messages: user + assistant + 2 tool results + final assistant"
        );

        // Verify both tool results
        let tool_results: Vec<&ToolResultMessage> = msgs
            .iter()
            .filter_map(|m| match m {
                AgentMessage::ToolResult(tr) => Some(tr),
                _ => None,
            })
            .collect();
        assert_eq!(tool_results.len(), 2, "Should have 2 tool results");
        assert_eq!(tool_results[0].tool_call_id, "tc_multi_a");
        assert_eq!(tool_results[1].tool_call_id, "tc_multi_b");
        assert_eq!(tool_results[0].tool_name, "echo");
        assert_eq!(tool_results[1].tool_name, "echo");

        // Verify the text content
        let text_a = tool_results[0].content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(text_a, Some("echo: alpha"));
        let text_b = tool_results[1].content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(text_b, Some("echo: beta"));
    }

    // ── Task 四: Tool result five states ──

    struct StatefulTool;

    impl Tool for StatefulTool {
        fn name(&self) -> &str {
            "stateful"
        }
        fn description(&self) -> &str {
            "Returns different states based on input"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string" }
                },
                "required": ["action"]
            })
        }
    }

    #[async_trait]
    impl AgentTool for StatefulTool {
        fn label(&self) -> &str {
            "stateful"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            let action = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
            match action {
                "ok" => Ok(AgentToolResult {
                    content: vec![Content::Text { text: "success".into() }],
                    ..Default::default()
                }),
                "error" => Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: "something went wrong".into(),
                    }],
                    is_error: true,
                    exit_code: Some(1),
                    ..Default::default()
                }),
                "timeout" => Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: "command timed out after 5 seconds".into(),
                    }],
                    is_error: true,
                    timed_out: true,
                    ..Default::default()
                }),
                "abort" => {
                    // Check if already aborted via context token
                    if context.cancellation.is_cancelled() {
                        return Ok(AgentToolResult {
                            content: vec![Content::Text { text: "aborted".into() }],
                            is_error: true,
                            aborted: true,
                            ..Default::default()
                        });
                    }
                    Ok(AgentToolResult {
                        content: vec![Content::Text {
                            text: "abort requested".into(),
                        }],
                        is_error: true,
                        aborted: true,
                        ..Default::default()
                    })
                }
                _ => Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: format!("unknown action: {}", action),
                    }],
                    ..Default::default()
                }),
            }
        }
    }

    async fn run_single_tool_action(action: &str) -> ToolResultMessage {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: format!("tc_{}", action),
                name: "stateful".into(),
                arguments: serde_json::json!({"action": action}),
                stop_reason: None,
            },
            MockStep::Text("done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(StatefulTool));
        agent.run(&format!("do {}", action)).await.unwrap();
        let msgs = agent.messages().await;
        msgs.into_iter()
            .find_map(|m| match m {
                AgentMessage::ToolResult(tr) => Some(tr),
                _ => None,
            })
            .expect("Should have a tool result")
    }

    #[tokio::test]
    async fn tool_result_state_ok() {
        let tr = run_single_tool_action("ok").await;
        assert!(!tr.is_error);
        let text = tr.content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(text, Some("success"));
    }

    #[tokio::test]
    async fn tool_result_state_error() {
        let tr = run_single_tool_action("error").await;
        assert!(tr.is_error);
        assert_eq!(tr.details.as_ref().and_then(|d| d["exit_code"].as_i64()), Some(1));
        let text = tr.content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert_eq!(text, Some("something went wrong"));
    }

    #[tokio::test]
    async fn tool_result_state_timeout() {
        let tr = run_single_tool_action("timeout").await;
        assert!(tr.is_error);
        let text = tr.content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert!(text.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn tool_result_state_aborted() {
        let tr = run_single_tool_action("abort").await;
        assert!(tr.is_error);
        let text = tr.content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert!(text.unwrap().contains("abort"));
    }

    // ── Task 五: Agent-level cancellation ──

    #[tokio::test]
    async fn agent_cancellation_aborts_long_running_tool() {
        use crate::ai::mock::MultiToolCallProvider;

        // Use a long-running bash command
        let shared_cwd = Arc::new(std::sync::RwLock::new(std::env::current_dir().unwrap()));
        let bash_tool = crate::coding_agent::tools::bash::BashTool::new(shared_cwd);

        let multi = MultiToolCallProvider::new(
            vec![MockStep::ToolCall {
                id: "tc_long".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "sleep 30"}),
                stop_reason: None,
            }],
            "should not reach here",
        );

        let mut agent = Agent::new(Box::new(multi), make_model());
        agent.add_tool(Box::new(bash_tool));

        // Clone cancellation token
        let token = agent.abort_flag();

        // Run the agent and abort after a short delay
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let token = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                token.cancel();
            });

            agent.run("sleep 30 seconds").await
        })
        .await;

        assert!(result.is_ok(), "Agent should finish within timeout after abort");
        let inner = result.unwrap();
        assert!(inner.is_ok(), "Agent should finish without error: {:?}", inner.err());
    }

    // ── CancellationToken isolation tests ──

    #[tokio::test]
    async fn agent_cancel_idle_produces_no_error() {
        let mock = MockProvider::text("never reached");
        let agent = Agent::new(Box::new(mock), make_model());
        let token = agent.abort_flag();
        token.cancel();
        // Agent hasn't started running, cancelling should be harmless
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn agent_new_run_not_affected_by_old_cancel() {
        let mock = MockProvider::new(vec![MockStep::Text("response".into())]);
        let mut agent = Agent::new(Box::new(mock), make_model());

        // Cancel the token before running
        let old_token = agent.abort_flag();
        old_token.cancel();
        assert!(old_token.is_cancelled());

        // Create a fresh token for the new run
        let new_token = CancellationToken::new();
        agent.set_abort_flag(new_token.clone());

        // The new run should not be affected by the old cancellation
        agent.run("hello").await.unwrap();
        let msgs = agent.messages().await;
        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected assistant message"),
        }
    }

    #[tokio::test]
    async fn agent_double_cancel_is_idempotent() {
        let token = CancellationToken::new();
        token.cancel();
        assert!(token.is_cancelled());
        // Second cancel should not panic
        token.cancel();
        assert!(token.is_cancelled());
    }

    // ── Task 六: DeepSeek history — tool-only assistant messages preserved ──

    #[test]
    fn deepseek_preserves_tool_only_assistant_messages() {
        use crate::ai::providers::deepseek::DeepSeekProvider;

        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("run ls".into()),
                timestamp: 1000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantContent::ToolCall {
                    id: "call_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                }],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2000,
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "call_1".into(),
                tool_name: "bash".into(),
                content: vec![TextOrImageContent::Text { text: "src".into() }],
                details: None,
                is_error: false,
                timestamp: 3000,
            }),
        ];

        let wire = DeepSeekProvider::build_messages(&messages, None);
        assert_eq!(wire.len(), 3, "All 3 messages should be present");
        assert_eq!(wire[0]["role"], "user");
        assert_eq!(wire[1]["role"], "assistant");
        assert!(
            wire[1]["tool_calls"].is_array(),
            "Assistant with tool_calls should have tool_calls field"
        );
        assert!(
            wire[1]["content"].is_null(),
            "Assistant with only tool calls should have null content"
        );
        assert_eq!(wire[2]["role"], "tool");
        assert_eq!(wire[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn deepseek_multi_turn_history_order() {
        use crate::ai::providers::deepseek::DeepSeekProvider;

        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("hello".into()),
                timestamp: 1000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantContent::Text {
                    text: "I'll help".into(),
                }],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: None,
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1100,
            }),
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("now run ls".into()),
                timestamp: 2000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantContent::ToolCall {
                    id: "call_2".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls"}),
                }],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2100,
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "call_2".into(),
                tool_name: "bash".into(),
                content: vec![TextOrImageContent::Text {
                    text: "file.txt".into(),
                }],
                details: None,
                is_error: false,
                timestamp: 2200,
            }),
        ];

        let wire = DeepSeekProvider::build_messages(&messages, None);
        assert_eq!(wire.len(), 5, "All 5 messages should be present in order");
        assert_eq!(wire[0]["role"], "user");
        assert_eq!(wire[1]["role"], "assistant");
        assert_eq!(wire[2]["role"], "user");
        assert_eq!(wire[3]["role"], "assistant");
        assert!(wire[3]["tool_calls"].is_array());
        assert_eq!(wire[4]["role"], "tool");
        assert_eq!(wire[4]["tool_call_id"], "call_2");
    }

    // ── Tool result semantic preservation in session ──

    #[tokio::test]
    async fn agent_tool_result_preserves_error_state_in_session() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_err".into(),
                name: "stateful".into(),
                arguments: serde_json::json!({"action": "error"}),
                stop_reason: None,
            },
            MockStep::Text("Got error".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(StatefulTool));
        agent.run("trigger error").await.unwrap();

        let msgs = agent.messages().await;
        let tool_result = msgs
            .iter()
            .find_map(|m| match m {
                AgentMessage::ToolResult(tr) => Some(tr),
                _ => None,
            })
            .expect("Should have tool result");

        assert!(tool_result.is_error, "Tool result should be marked as error");
        assert_eq!(tool_result.tool_call_id, "tc_err");
        assert_eq!(tool_result.tool_name, "stateful");
        let details = tool_result.details.as_ref().unwrap();
        assert_eq!(details["exit_code"], 1);
    }

    // ── Structured error tool (not just exit code) ──

    struct StructuredErrorTool;

    impl Tool for StructuredErrorTool {
        fn name(&self) -> &str {
            "structured_err"
        }
        fn description(&self) -> &str {
            "Returns structured error details"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string" }
                },
                "required": ["kind"]
            })
        }
    }

    #[async_trait]
    impl AgentTool for StructuredErrorTool {
        fn label(&self) -> &str {
            "structured_err"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> anyhow::Result<AgentToolResult> {
            let kind = params.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            match kind {
                "structured" => Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: "Permission denied: /root/secret".into(),
                    }],
                    is_error: true,
                    details: serde_json::json!({
                        "error_type": "permission_denied",
                        "path": "/root/secret",
                        "suggestion": "Run with sudo or check file permissions"
                    }),
                    ..Default::default()
                }),
                "all_fields" => Ok(AgentToolResult {
                    content: vec![Content::Text {
                        text: "error with all fields".into(),
                    }],
                    is_error: true,
                    exit_code: Some(127),
                    timed_out: true,
                    aborted: false,
                    details: serde_json::json!({ "custom": "data" }),
                    ..Default::default()
                }),
                _ => Ok(AgentToolResult {
                    content: vec![Content::Text { text: "ok".into() }],
                    ..Default::default()
                }),
            }
        }
    }

    #[tokio::test]
    async fn tool_structured_error_preserves_custom_details() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_struct".into(),
                name: "structured_err".into(),
                arguments: serde_json::json!({"kind": "structured"}),
                stop_reason: None,
            },
            MockStep::Text("handled".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(StructuredErrorTool));
        agent.run("trigger structured error").await.unwrap();

        let msgs = agent.messages().await;
        let tool_result = msgs
            .iter()
            .find_map(|m| match m {
                AgentMessage::ToolResult(tr) => Some(tr),
                _ => None,
            })
            .expect("Should have tool result");

        assert!(tool_result.is_error);
        let details = tool_result.details.as_ref().unwrap();
        assert_eq!(details["error_type"], "permission_denied");
        assert_eq!(details["path"], "/root/secret");
        assert_eq!(details["suggestion"], "Run with sudo or check file permissions");

        // Text content should contain the error description
        let text = tool_result.content.iter().find_map(|c| match c {
            TextOrImageContent::Text { text } => Some(text.as_str()),
            _ => None,
        });
        assert!(text.unwrap().contains("Permission denied"));
    }

    #[tokio::test]
    async fn tool_result_all_error_fields_present_in_details() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_all".into(),
                name: "structured_err".into(),
                arguments: serde_json::json!({"kind": "all_fields"}),
                stop_reason: None,
            },
            MockStep::Text("handled".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(StructuredErrorTool));
        agent.run("trigger all fields").await.unwrap();

        let msgs = agent.messages().await;
        let tool_result = msgs
            .iter()
            .find_map(|m| match m {
                AgentMessage::ToolResult(tr) => Some(tr),
                _ => None,
            })
            .expect("Should have tool result");

        assert!(tool_result.is_error);
        let details = tool_result.details.as_ref().unwrap();
        assert_eq!(details["exit_code"], 127);
        assert_eq!(details["timed_out"], true);
        assert_eq!(details["custom"], "data");
    }

    #[tokio::test]
    async fn tool_terminate_stops_agent_without_second_model_request() {
        // Tool returns terminate=true. Agent should stop immediately
        // after executing the tool, without making another LLM call.
        let mock = MockProvider::new(vec![MockStep::ToolCall {
            id: "tc_term".into(),
            name: "terminator".into(),
            arguments: serde_json::json!({}),
            stop_reason: None,
        }]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(TerminatorTool));
        agent.run("terminate now").await.unwrap();

        let msgs = agent.messages().await;
        // user + assistant(tool_call) + tool_result = 3 messages
        // No second assistant message (no second LLM call)
        assert_eq!(msgs.len(), 3, "Should have exactly 3 messages (no second LLM call)");
        assert!(matches!(&msgs[0], AgentMessage::User(_)));
        assert!(matches!(&msgs[1], AgentMessage::Assistant(a) if a.stop_reason == StopReason::ToolUse));
        assert!(matches!(&msgs[2], AgentMessage::ToolResult(_)));
    }
}
