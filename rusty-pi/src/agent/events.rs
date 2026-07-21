//! Unified event interface for agent run lifecycle.
//!
//! `AgentEvent` provides a single stream of events that consumers (UI, logging,
//! testing) can subscribe to. The agent never writes to stdout/stderr directly;
//! all output goes through this channel.

use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;

/// Opaque identifier for a single agent run.
///
/// Every event carries a `run_id` so consumers can ignore late events
/// from a cancelled run that arrive after a new run has started.
///
/// Internally implemented as a monotonically increasing `u64` wrapped
/// in a newtype for type safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(pub u64);

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "run-{}", self.0)
    }
}

/// A single event emitted during an agent run.
///
/// Every event carries a `run_id` so that:
/// - Late events from a cancelled run are silently ignored by the TUI.
/// - Tool identity (tool_call_id + name) is always explicit.
/// - Events can be traced to their originating run.
///
/// Events are emitted in a strict order within a run:
/// 1. `RunStarted`
/// 2. Zero or more text/tool events
/// 3. Exactly one terminal event: `RunFinished` or `RunAborted`
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A new agent run has started.
    RunStarted { run_id: RunId },

    /// A chunk of text from the LLM response.
    TextDelta { run_id: RunId, text: String },

    /// A chunk of thinking/reasoning content from the LLM.
    ThinkingDelta { run_id: RunId, text: String },

    /// A tool call has started executing.
    ToolStarted {
        run_id: RunId,
        tool_call_id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Streaming output from a tool (stdout or stderr).
    ToolOutput {
        run_id: RunId,
        tool_call_id: String,
        stream: ToolOutputStream,
        chunk: String,
    },

    /// A tool call has finished.
    ToolFinished {
        run_id: RunId,
        tool_call_id: String,
        name: String,
        result: AgentToolResult,
    },

    /// An error from the LLM provider.
    ProviderError { run_id: RunId, error: ProviderError },

    /// The run was cancelled by the user.
    RunAborted { run_id: RunId },

    /// The run finished normally.
    RunFinished { run_id: RunId, stop_reason: StopReason },
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
        let event = AgentEvent::TextDelta {
            run_id: RunId(1),
            text: "hello".into(),
        };
        let cloned = event.clone();
        match cloned {
            AgentEvent::TextDelta { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn tool_output_stream_distinction() {
        assert_ne!(ToolOutputStream::Stdout, ToolOutputStream::Stderr);
    }

    #[test]
    fn run_id_display() {
        assert_eq!(RunId(42).to_string(), "run-42");
    }

    #[test]
    fn run_id_equality() {
        assert_eq!(RunId(1), RunId(1));
        assert_ne!(RunId(1), RunId(2));
    }

    #[test]
    fn run_id_ordering() {
        assert!(RunId(1) < RunId(2));
        assert!(RunId(10) > RunId(5));
    }
}
