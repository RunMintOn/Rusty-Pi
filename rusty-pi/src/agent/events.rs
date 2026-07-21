//! Unified event interface for agent run lifecycle.
//!
//! `AgentEvent` provides a single stream of events that consumers (UI, logging,
//! testing) can subscribe to. The agent never writes to stdout/stderr directly;
//! all output goes through this channel.

use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;

/// A single event emitted during an agent run.
///
/// Events are emitted in a strict order:
/// 1. `RunStarted`
/// 2. Zero or more text/tool events
/// 3. Exactly one terminal event: `RunFinished` or `RunAborted`
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A new agent run has started.
    RunStarted,

    /// A chunk of text from the LLM response.
    TextDelta { text: String },

    /// A chunk of thinking/reasoning content from the LLM.
    ThinkingDelta { text: String },

    /// A tool call has started executing.
    ToolStarted {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Streaming output from a tool (stdout or stderr).
    ToolOutput {
        id: String,
        stream: ToolOutputStream,
        chunk: String,
    },

    /// A tool call has finished.
    ToolFinished {
        id: String,
        name: String,
        result: AgentToolResult,
    },

    /// An error from the LLM provider.
    ProviderError { error: ProviderError },

    /// The run was cancelled by the user.
    RunAborted,

    /// The run finished normally.
    RunFinished { stop_reason: StopReason },
}

/// Which stream a tool output chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
}

/// An error from the LLM provider.
#[derive(Debug, Clone)]
pub struct ProviderError {
    /// The stop reason (always `StopReason::Error`).
    pub reason: StopReason,
    /// Human-readable error message.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_is_clone() {
        let event = AgentEvent::TextDelta { text: "hello".into() };
        let cloned = event.clone();
        match cloned {
            AgentEvent::TextDelta { text } => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn tool_output_stream_distinction() {
        assert_ne!(ToolOutputStream::Stdout, ToolOutputStream::Stderr);
    }
}
