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

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{AgentMessage, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use std::sync::Mutex;

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
}

impl MockProvider {
    /// Create a provider that returns the given sequence of steps.
    pub fn new(steps: Vec<MockStep>) -> Self {
        Self { steps: Mutex::new(steps) }
    }

    /// Create a provider that returns a single text response.
    ///
    /// Shorthand for `MockProvider::new(vec![MockStep::Text(text.to_string())])`.
    pub fn text(text: &str) -> Self {
        Self::new(vec![MockStep::Text(text.to_string())])
    }
}

#[async_trait]
impl ProviderApi for MockProvider {
    async fn stream(
        &self,
        _model: &Model,
        _messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<StreamReceiver> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let mut steps = self.steps.lock().unwrap();
        let step = steps.first().cloned();
        if steps.len() > 1 {
            steps.remove(0);
        }
        let step = step.unwrap_or(MockStep::Text("(done)".into()));

        tokio::spawn(async move {
            match step {
                MockStep::Text(text) => {
                    for word in text.split(' ') {
                        let chunk = format!("{} ", word);
                        if tx.send(StreamEvent::TextDelta { delta: chunk }).await.is_err() { return; }
                    }
                    let acc = MessageAccumulator::new("mock", "mock", "mock");
                    let msg = acc.build();
                    let _ = tx.send(StreamEvent::Done { message: msg }).await;
                }
                MockStep::ToolCall { id, name, arguments, stop_reason } => {
                    let _ = tx.send(StreamEvent::ToolCall { id, name: name.clone(), arguments: arguments.clone() }).await;
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
                    let _ = tx.send(StreamEvent::Done { message: msg }).await;
                }
                MockStep::Error(msg) => {
                    let _ = tx.send(StreamEvent::Error { reason: StopReason::Error, message: msg }).await;
                }
            }
        });

        Ok(rx)
    }
}
