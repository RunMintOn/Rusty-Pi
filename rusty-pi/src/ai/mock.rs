//! Mock provider for testing — mirrors the original "faux provider".
//!
//! Returns predetermined responses instead of calling a real LLM API.
//! Supports text responses, tool call sequences, and error scenarios.

use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use std::sync::Mutex;

/// A predetermined response step that the mock provider returns.
#[derive(Debug, Clone)]
pub enum MockStep {
    /// Return a plain text response and stop.
    Text(String),
    /// Return a tool call, expecting the agent to execute it and feed back results.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Return an error message.
    Error(String),
}

/// Mock provider that replays predetermined response steps.
///
/// On each call to `stream()`:
/// - If steps remain, returns the next step as an `AssistantMessage`.
/// - If no steps remain, returns a default "done" response.
pub struct MockProvider {
    steps: Mutex<Vec<MockStep>>,
}

impl MockProvider {
    /// Create a new mock provider with the given response steps.
    pub fn new(steps: Vec<MockStep>) -> Self {
        Self {
            steps: Mutex::new(steps),
        }
    }

    /// Create a mock provider that returns a single text response.
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
    ) -> anyhow::Result<AssistantMessage> {
        let mut steps = self.steps.lock().unwrap();
        let step = steps.first().cloned();
        // Keep the last step for repeated calls during multi-turn loops.
        if steps.len() > 1 {
            steps.remove(0);
        }

        match step.unwrap_or(MockStep::Text("(done)".into())) {
            MockStep::Text(text) => Ok(AssistantMessage {
                content: vec![AssistantContent::Text { text }],
                api: "mock".into(),
                provider: "mock".into(),
                model: "mock".into(),
                usage: None,
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1000,
            }),
            MockStep::ToolCall { id, name, arguments } => Ok(AssistantMessage {
                content: vec![AssistantContent::ToolCall { id, name, arguments }],
                api: "mock".into(),
                provider: "mock".into(),
                model: "mock".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2000,
            }),
            MockStep::Error(msg) => Ok(AssistantMessage {
                content: vec![],
                api: "mock".into(),
                provider: "mock".into(),
                model: "mock".into(),
                usage: None,
                stop_reason: StopReason::Error,
                error_message: Some(msg),
                timestamp: 3000,
            }),
        }
    }
}
