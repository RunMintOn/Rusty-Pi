//! Unified event interface for agent run lifecycle.
//!
//! `AgentEvent` provides a single stream of events that consumers (UI, logging,
//! testing) can subscribe to. The agent never writes to stdout/stderr directly;
//! all output goes through this channel.

use crate::agent::types::AgentToolResult;
use crate::ai::types::StopReason;
use serde::{Deserialize, Serialize};

/// Opaque identifier for a single agent run.
///
/// Every event carries a `run_id` so consumers can ignore late events
/// from a cancelled run that arrive after a new run has started.
///
/// Internally implemented as a monotonically increasing `u64` wrapped
/// in a newtype for type safety. The inner value is private; use
/// [`RunId::new`] to construct and [`RunId::get`] to inspect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RunId(u64);

impl RunId {
    /// Create a new RunId from a raw integer.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Get the underlying integer value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

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

    /// The run failed due to an internal agent error.
    RunFailed { run_id: RunId, error: AgentRunError },
}

/// Phase where an agent run failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentRunPhase {
    /// Failed to create or start the provider stream.
    ProviderStart,
    /// Failed while receiving from the provider stream.
    ProviderStream,
    /// Failed to append a message to the session.
    Session,
    /// Failed during tool execution.
    ToolExecution,
    /// General agent loop invariant failure.
    AgentLoop,
}

impl std::fmt::Display for AgentRunPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProviderStart => write!(f, "provider start"),
            Self::ProviderStream => write!(f, "provider stream"),
            Self::Session => write!(f, "session"),
            Self::ToolExecution => write!(f, "tool execution"),
            Self::AgentLoop => write!(f, "agent loop"),
        }
    }
}

/// Structured error for agent run failures.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentRunError {
    /// The phase where the failure occurred.
    pub phase: AgentRunPhase,
    /// Human-readable error message.
    pub message: String,
}

impl std::fmt::Display for AgentRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.phase, self.message)
    }
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
            run_id: RunId::new(1),
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
        assert_eq!(RunId::new(42).to_string(), "run-42");
    }

    #[test]
    fn run_id_get_returns_value_without_exposing_field() {
        assert_eq!(RunId::new(42).get(), 42);
    }

    #[test]
    fn run_id_equality() {
        assert_eq!(RunId::new(1), RunId::new(1));
        assert_ne!(RunId::new(1), RunId::new(2));
    }

    #[test]
    fn run_id_ordering() {
        assert!(RunId::new(1) < RunId::new(2));
        assert!(RunId::new(10) > RunId::new(5));
    }
}
