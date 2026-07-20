//! Agent-specific types: AgentTool, AgentToolResult, execution mode.
//!
//! Mirrors `@earendil-works/pi-agent-core/src/types.ts`.

use crate::ai::types::{Content, Tool};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Execution mode for a tool — sequential or parallel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ToolExecutionMode {
    /// This tool must execute one at a time with other tool calls.
    Sequential,
    /// This tool can execute concurrently with other tool calls.
    Parallel,
}

/// Result produced by a tool execution.
///
/// Mirrors `AgentToolResult` in the original `@earendil-works/pi-agent-core`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult<T = serde_json::Value> {
    /// Text or image content returned to the model.
    pub content: Vec<Content>,
    /// Arbitrary structured details for logs or UI rendering.
    pub details: T,
    /// Names of tools introduced by this result, available from this point onward.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    /// Hint that the agent should stop after the current tool batch.
    #[serde(default)]
    pub terminate: bool,
    /// Whether the tool execution resulted in an error.
    #[serde(default)]
    pub is_error: bool,
    /// Process exit code, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Whether the command was killed due to timeout.
    #[serde(default)]
    pub timed_out: bool,
    /// Whether the command was aborted by the user.
    #[serde(default)]
    pub aborted: bool,
}

impl Default for AgentToolResult {
    fn default() -> Self {
        Self {
            content: Vec::new(),
            details: serde_json::Value::Null,
            added_tool_names: None,
            terminate: false,
            is_error: false,
            exit_code: None,
            timed_out: false,
            aborted: false,
        }
    }
}

/// A tool that can be executed by the agent.
///
/// Mirrors `AgentTool` in the original `@earendil-works/pi-agent-core/src/types.ts`.
/// This extends the base `Tool` trait (schema-only) with execution capabilities.
#[async_trait]
pub trait AgentTool: Tool + Send + Sync {
    /// Human-readable label for UI display.
    fn label(&self) -> &str;

    /// Optional compatibility shim for raw tool-call arguments before schema validation.
    /// Must return an object that matches the tool's parameter schema.
    fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        args
    }

    /// Execute the tool call. Throw on failure instead of encoding errors in `content`.
    async fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        signal: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> anyhow::Result<AgentToolResult>;

    /// Per-tool execution mode override.
    fn execution_mode(&self) -> ToolExecutionMode {
        ToolExecutionMode::Sequential
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::Tool;

    struct TestTool;

    impl Tool for TestTool {
        fn name(&self) -> &str {
            "test"
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {
                        "type": "string"
                    }
                }
            })
        }
    }

    #[async_trait]
    impl AgentTool for TestTool {
        fn label(&self) -> &str {
            "test"
        }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
        ) -> anyhow::Result<AgentToolResult> {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "ok".into() }],
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn test_tool_execute_roundtrip() {
        let tool = TestTool;
        assert_eq!(tool.name(), "test");
        assert_eq!(tool.description(), "A test tool");
        assert_eq!(tool.label(), "test");

        let result = tool
            .execute("call_1", serde_json::json!({"input": "hello"}), None)
            .await
            .unwrap();
        assert_eq!(result.content.len(), 1);
    }

    #[test]
    fn test_tool_execution_mode_default() {
        let tool = TestTool;
        assert_eq!(tool.execution_mode(), ToolExecutionMode::Sequential);
    }
}
