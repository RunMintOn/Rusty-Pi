//! Mock provider for testing.
//!
//! Provides a `MockProvider` that returns preset responses instead of calling
//! a real LLM. Used to test the agent loop and tool execution without network
//! access.
//!
//! # Behaviour contract
//!
//! * Each call to [`ProviderApi::stream`] consumes one step from the sequence
//!   (first in, first out). When the sequence is exhausted, the last step is
//!   repeated.
//! * `Text` steps emit the text as word-by-word [`StreamEvent::TextDelta`]
//!   events and finish with `Done`.
//! * `ToolCall` steps emit a single [`StreamEvent::ToolCall`] event and finish
//!   with `Done` with `StopReason::Stop` (the agent engine overrides this to
//!   `ToolUse` when tool calls are present).
//! * `Error` steps emit [`StreamEvent::Error`] and terminate.
//! * All fields (`api`, `provider`, `model`) in the emitted `Done` message
//!   are set to `"mock"`.

use crate::ai::providers::{Model, ProviderApi, ProviderRequestContext, ProviderStream};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{AgentMessage, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// A single step in a mock provider response sequence.
#[derive(Debug, Clone)]
pub enum MockStep {
    /// Emit the given text as word-by-word deltas, then signal `Done`.
    Text(String),
    /// Emit a single tool call, then signal `Done`.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        /// Custom stop reason for the Done event (default: Stop).
        /// Use `Some(StopReason::Length)` to simulate length-truncated responses.
        stop_reason: Option<StopReason>,
    },
    /// Signal an error immediately.
    Error(String),
}

/// Mock LLM provider that returns preset response sequences.
///
/// Each call to [`ProviderApi::stream`] consumes one step from the sequence.
/// Thread-safe via internal mutability (`Mutex`).
pub struct MockProvider {
    steps: Mutex<Vec<MockStep>>,
    /// Captured messages from each `stream()` call.
    captured_requests: Arc<Mutex<Vec<Vec<AgentMessage>>>>,
}

impl MockProvider {
    /// Create a provider that returns the given sequence of steps.
    pub fn new(steps: Vec<MockStep>) -> Self {
        Self {
            steps: Mutex::new(steps),
            captured_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return all messages captured by `stream()` calls, in order.
    pub fn captured_requests(&self) -> Vec<Vec<AgentMessage>> {
        self.captured_requests.lock().unwrap().clone()
    }

    /// Return the Arc<Mutex<...>> for capturing requests (useful when provider is boxed).
    pub fn captured_requests_arc(&self) -> Arc<Mutex<Vec<Vec<AgentMessage>>>> {
        self.captured_requests.clone()
    }

    /// Return the messages from the Nth `stream()` call (0-indexed).
    pub fn captured_request(&self, index: usize) -> Vec<AgentMessage> {
        self.captured_requests
            .lock()
            .unwrap()
            .get(index)
            .cloned()
            .unwrap_or_default()
    }

    /// Create a provider that returns a single text response.
    ///
    /// Shorthand for `MockProvider::new(vec![MockStep::Text(text.to_string())])`.
    pub fn text(text: &str) -> Self {
        Self::new(vec![MockStep::Text(text.to_string())])
    }
}

/// A provider that emits multiple tool calls in one response,
/// then returns final text on the second call.
///
/// Used to test that the agent handles multiple tool calls correctly.
pub struct MultiToolCallProvider {
    /// Tool calls to emit on the first call.
    calls: Vec<MockStep>,
    /// Final text response after all tool calls (returned on second call).
    final_text: String,
    /// Captured messages.
    captured_requests: Arc<Mutex<Vec<Vec<AgentMessage>>>>,
    /// Track whether we've already emitted tool calls.
    emitted: Arc<Mutex<bool>>,
}

impl MultiToolCallProvider {
    pub fn new(calls: Vec<MockStep>, final_text: &str) -> Self {
        Self {
            calls,
            final_text: final_text.to_string(),
            captured_requests: Arc::new(Mutex::new(Vec::new())),
            emitted: Arc::new(Mutex::new(false)),
        }
    }

    pub fn captured_requests(&self) -> Vec<Vec<AgentMessage>> {
        self.captured_requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ProviderApi for MultiToolCallProvider {
    async fn stream(
        &self,
        _model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
        _system_prompt: Option<&str>,
        context: ProviderRequestContext,
    ) -> anyhow::Result<ProviderStream> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        self.captured_requests.lock().unwrap().push(messages.to_vec());
        let cancellation = context.cancellation.child_token();
        let producer_cancellation = cancellation.clone();

        let already_emitted = *self.emitted.lock().unwrap();
        if already_emitted {
            // Second call: return final text
            let final_text = self.final_text.clone();
            let producer_handle = tokio::spawn(async move {
                for word in final_text.split(' ') {
                    let chunk = format!("{} ", word);
                    if !send_event(&tx, StreamEvent::TextDelta { delta: chunk }, &producer_cancellation).await {
                        return;
                    }
                }
                let acc = MessageAccumulator::new("mock", "mock", "mock");
                let msg = acc.build();
                let _ = send_event(&tx, StreamEvent::Done { message: msg }, &producer_cancellation).await;
            });
            return Ok(ProviderStream::new(rx, producer_handle, cancellation));
        } else {
            // First call: emit tool calls
            *self.emitted.lock().unwrap() = true;
            let calls = self.calls.clone();
            let producer_handle = tokio::spawn(async move {
                for step in &calls {
                    if let MockStep::ToolCall {
                        id,
                        name,
                        arguments,
                        stop_reason: _,
                    } = step
                        && !send_event(
                            &tx,
                            StreamEvent::ToolCall {
                                id: id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                            },
                            &producer_cancellation,
                        )
                        .await
                    {
                        return;
                    }
                }
                let acc = MessageAccumulator::new("mock", "mock", "mock");
                let msg = acc.build();
                let _ = send_event(&tx, StreamEvent::Done { message: msg }, &producer_cancellation).await;
            });
            return Ok(ProviderStream::new(rx, producer_handle, cancellation));
        }
    }
}

#[async_trait]
impl ProviderApi for MockProvider {
    async fn stream(
        &self,
        _model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
        _system_prompt: Option<&str>,
        context: ProviderRequestContext,
    ) -> anyhow::Result<ProviderStream> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        // Capture messages for test assertions
        self.captured_requests.lock().unwrap().push(messages.to_vec());
        let mut steps = self.steps.lock().unwrap();
        let step = steps.first().cloned();
        if steps.len() > 1 {
            steps.remove(0);
        }
        let step = step.unwrap_or(MockStep::Text("(done)".into()));
        let cancellation = context.cancellation.child_token();
        let producer_cancellation = cancellation.clone();

        let producer_handle = tokio::spawn(async move {
            match step {
                MockStep::Text(text) => {
                    for word in text.split(' ') {
                        let chunk = format!("{} ", word);
                        if !send_event(&tx, StreamEvent::TextDelta { delta: chunk }, &producer_cancellation).await {
                            return;
                        }
                    }
                    let acc = MessageAccumulator::new("mock", "mock", "mock");
                    let msg = acc.build();
                    let _ = send_event(&tx, StreamEvent::Done { message: msg }, &producer_cancellation).await;
                }
                MockStep::ToolCall {
                    id,
                    name,
                    arguments,
                    stop_reason,
                } => {
                    if !send_event(
                        &tx,
                        StreamEvent::ToolCall {
                            id,
                            name: name.clone(),
                            arguments: arguments.clone(),
                        },
                        &producer_cancellation,
                    )
                    .await
                    {
                        return;
                    }
                    let mut acc = MessageAccumulator::new("mock", "mock", "mock");
                    if let Some(reason) = stop_reason {
                        use crate::ai::stream::StreamEvent;
                        acc.push(StreamEvent::Done {
                            message: AssistantMessage {
                                content: vec![],
                                api: "mock".into(),
                                provider: "mock".into(),
                                model: "mock".into(),
                                usage: None,
                                stop_reason: reason,
                                error_message: None,
                                timestamp: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as i64,
                            },
                        });
                    }
                    let msg = acc.build();
                    let _ = send_event(&tx, StreamEvent::Done { message: msg }, &producer_cancellation).await;
                }
                MockStep::Error(msg) => {
                    let _ = send_event(
                        &tx,
                        StreamEvent::Error {
                            reason: StopReason::Error,
                            message: msg,
                        },
                        &producer_cancellation,
                    )
                    .await;
                }
            }
        });

        Ok(ProviderStream::new(rx, producer_handle, cancellation))
    }
}

async fn send_event(
    tx: &tokio::sync::mpsc::Sender<StreamEvent>,
    event: StreamEvent,
    cancellation: &tokio_util::sync::CancellationToken,
) -> bool {
    if cancellation.is_cancelled() {
        return false;
    }
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => false,
        result = tx.send(event) => result.is_ok(),
    }
}
