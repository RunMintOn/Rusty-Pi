//! Streaming event types for LLM provider responses.
//!
//! Mirrors the `AssistantMessageEvent` type from the original pi.
//! Events are sent through a `tokio::sync::mpsc::Receiver` to allow
//! the agent loop and UI to process tokens as they arrive.

use crate::ai::types::{AssistantMessage, StopReason};

/// An event emitted during LLM streaming.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// A text delta (part of the response being streamed).
    TextDelta {
        delta: String,
    },
    /// A complete tool call was received.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// Streaming finished successfully.
    Done {
        message: AssistantMessage,
    },
    /// Streaming finished with an error.
    Error {
        reason: StopReason,
        message: String,
    },
}

/// Buffer that accumulates stream events into a complete AssistantMessage.
#[derive(Debug)]
pub struct MessageAccumulator {
    text_parts: Vec<String>,
    tool_calls: Vec<(String, String, serde_json::Value)>,
    api: String,
    provider: String,
    model: String,
    stop_reason: StopReason,
    error_message: Option<String>,
    timestamp: i64,
}

impl MessageAccumulator {
    pub fn new(api: &str, provider: &str, model: &str) -> Self {
        Self {
            text_parts: Vec::new(),
            tool_calls: Vec::new(),
            api: api.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        }
    }

    /// Process a stream event, updating internal state.
    pub fn push(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::TextDelta { delta } => {
                self.text_parts.push(delta);
            }
            StreamEvent::ToolCall { id, name, arguments } => {
                self.tool_calls.push((id, name, arguments));
            }
            StreamEvent::Done { message } => {
                self.stop_reason = message.stop_reason;
                self.error_message = message.error_message;
            }
            StreamEvent::Error { reason, message } => {
                self.stop_reason = reason;
                self.error_message = Some(message);
            }
        }
    }

    /// Build the final AssistantMessage.
    pub fn build(self) -> AssistantMessage {
        use crate::ai::types::AssistantContent;

        let mut content = Vec::new();

        let full_text = self.text_parts.join("");
        if !full_text.is_empty() {
            content.push(AssistantContent::Text { text: full_text });
        }

        for (id, name, arguments) in self.tool_calls {
            content.push(AssistantContent::ToolCall { id, name, arguments });
        }

        AssistantMessage {
            content,
            api: self.api,
            provider: self.provider,
            model: self.model,
            usage: None,
            stop_reason: self.stop_reason,
            error_message: self.error_message,
            timestamp: self.timestamp,
        }
    }
}
