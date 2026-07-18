//! Agent — the core loop that drives LLM ↔ tool interactions.

use crate::agent::session::Session;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{
    AgentMessage, AgentToolCall, AssistantContent, Content, MessageContent, StopReason,
    TextOrImageContent, Tool, ToolResultMessage, UserMessage,
};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Callback for streaming text deltas.
pub type TextCallback = Box<dyn FnMut(&str) + Send>;

/// Shared abort flag for signalling Ctrl+C / cancellation.
pub type AbortFlag = Arc<AtomicBool>;

/// The agent that orchestrates the LLM → tool → LLM loop.
pub struct Agent {
    session: Session,
    tools: Vec<Box<dyn AgentTool>>,
    provider: Box<dyn ProviderApi>,
    model: Model,
    config: AgentConfig,
    /// Optional callback for streaming text deltas.
    on_text: Option<TextCallback>,
    /// Shared flag: when true, the agent should abort the current round.
    abort: AbortFlag,
}

impl Agent {
    pub fn new(provider: Box<dyn ProviderApi>, model: Model) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            session: Session::new(cwd),
            tools: Vec::new(),
            provider,
            model,
            config: AgentConfig::default(),
            on_text: None,
            abort: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Register a callback for streaming text deltas.
    /// Register a callback for streaming text deltas.
    pub fn on_text<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_text = Some(Box::new(callback));
    }

    /// Signal the agent to abort the current round.
    /// The next tick of the stream loop will notice and return `StopReason::Aborted`.
    pub fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }

    /// Replace the abort flag (used to share a flag between REPL and agent).
    pub fn set_abort_flag(&mut self, flag: AbortFlag) {
        self.abort = flag;
    }

    /// Get a reference to the abort flag (for sharing with the REPL).
    pub fn abort_flag(&self) -> AbortFlag {
        self.abort.clone()
    }

    pub fn add_tool(&mut self, tool: Box<dyn AgentTool>) {
        self.tools.push(tool);
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt;
    }

    pub fn messages(&self) -> Vec<&AgentMessage> {
        self.session.messages()
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    fn tool_refs(&self) -> Vec<&dyn Tool> {
        self.tools.iter().map(|t| t.as_ref() as &dyn Tool).collect()
    }

    pub async fn run(&mut self, prompt: &str) -> Result<()> {
        let now = Self::now_ms();

        let user_msg = AgentMessage::User(UserMessage {
            content: MessageContent::Text(prompt.to_string()),
            timestamp: now,
        });
        self.session.add_message(user_msg);

        for round in 0..=self.config.max_tool_rounds {
            let tool_refs = self.tool_refs();
            let current_messages: Vec<AgentMessage> = self
                .session
                .messages()
                .into_iter()
                .cloned()
                .collect();

            let mut rx = self
                .provider
                .stream(&self.model, &current_messages, &tool_refs)
                .await
                .with_context(|| format!("LLM call failed at round {}", round))?;

            // Accumulate stream events into an AssistantMessage
            let mut acc = MessageAccumulator::new(
                self.model.api,
                self.model.id,
                self.model.id,
            );

            // Check for abort BEFORE starting the stream loop
            if self.abort.load(Ordering::SeqCst) {
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
                self.session.add_message(AgentMessage::Assistant(msg));
                return Ok(());
            }

            while let Some(event) = rx.recv().await {
                // Fire streaming callback for text deltas
                if let StreamEvent::TextDelta { ref delta } = event
                    && let Some(ref mut cb) = self.on_text {
                        cb(delta);
                }

                let is_terminal = matches!(event, StreamEvent::Done { .. } | StreamEvent::Error { .. });
                acc.push(event);
                if is_terminal {
                    break;
                }
            }

            // Check for abort AFTER the stream loop (user hit Ctrl+C during streaming)
            if self.abort.load(Ordering::SeqCst) {
                // Override response with aborted status
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
                self.session.add_message(AgentMessage::Assistant(msg));
                return Ok(());
            }

            let response = acc.build();

            // Extract tool calls as AgentToolCall structs
            let tool_calls: Vec<AgentToolCall> = response.content.iter()
                .filter_map(|c| match c {
                    AssistantContent::ToolCall { id, name, arguments } => {
                        Some(AgentToolCall { id: id.clone(), name: name.clone(), arguments: arguments.clone() })
                    }
                    _ => None,
                }).collect();

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

            self.session
                .add_message(AgentMessage::Assistant(response.clone()));

            match response.stop_reason {
                StopReason::Stop | StopReason::Length | StopReason::Error | StopReason::Aborted => {
                    return Ok(());
                }
                StopReason::ToolUse => {
                    if round >= self.config.max_tool_rounds {
                        anyhow::bail!(
                            "Exceeded maximum tool call rounds ({})",
                            self.config.max_tool_rounds
                        );
                    }

                    let mut any_terminate = false;
                    for call in &tool_calls {
                        let (tool_result, terminate) = self
                            .execute_tool(&call.id, &call.name, call.arguments.clone())
                            .await
                            .with_context(|| format!("Tool '{}' execution failed", call.name))?;
                        self.session.add_message(tool_result);
                        if terminate {
                            any_terminate = true;
                        }
                    }
                    if any_terminate {
                        return Ok(());
                    }
                }
            }
        }

        anyhow::bail!("Agent loop exited without producing a final response")
    }

    /// Execute a tool and return (AgentMessage, terminate_flag).
    async fn execute_tool(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<(AgentMessage, bool)> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == tool_name)
            .with_context(|| format!("Tool '{}' not found", tool_name))?;

        let result = tool
            .execute(tool_call_id, args, None)
            .await
            .with_context(|| format!("Tool '{}' execution failed", tool_name))?;

        let now = Self::now_ms();

        let content: Vec<_> = result
            .content
            .into_iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(TextOrImageContent::Text { text }),
                Content::Image { data, mime_type } => {
                    Some(TextOrImageContent::Image { data, mime_type })
                }
                _ => None,
            })
            .collect();

        Ok((AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content,
            details: Some(serde_json::json!({})),
            is_error: false,
            timestamp: now,
        }), result.terminate))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::types::AgentToolResult;
    use crate::ai::mock::{MockProvider, MockStep};
    use crate::ai::providers::Model;
    use async_trait::async_trait;

    struct EchoTool;

    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echoes the input back" }
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
        fn label(&self) -> &str { "echo" }

        async fn execute(
            &self,
            _tool_call_id: &str,
            params: serde_json::Value,
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
        ) -> anyhow::Result<AgentToolResult> {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            Ok(AgentToolResult {
                content: vec![Content::Text { text: format!("echo: {}", text) }],
                ..Default::default()
            })
        }
    }

    fn make_model() -> Model { Model { id: "mock", api: "mock" } }

    #[tokio::test]
    async fn agent_returns_text_response() {
        let mock = MockProvider::text("Hello from mock!");
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.run("Hi there").await.unwrap();
        let msgs = agent.messages();
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
            MockStep::ToolCall { id: "tc_1".into(), name: "echo".into(), arguments: serde_json::json!({"text": "hello"}), stop_reason: None },
            MockStep::Text("Done!".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Run echo").await.unwrap();
        let msgs = agent.messages();
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
        let msgs = agent.messages();
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
            MockStep::ToolCall { id: "tc_bash".into(), name: "bash".into(), arguments: serde_json::json!({"command": "echo bash-works"}), stop_reason: None },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(crate::coding_agent::tools::bash::BashTool::new(
            std::env::current_dir().unwrap().to_string_lossy().to_string(),
        )));
        agent.run("Run bash").await.unwrap();
        let msgs = agent.messages();
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
            MockStep::ToolCall { id: "tc_1".into(), name: "echo".into(), arguments: serde_json::json!({"text": "first"}), stop_reason: None },
            MockStep::ToolCall { id: "tc_2".into(), name: "echo".into(), arguments: serde_json::json!({"text": "second"}), stop_reason: None },
            MockStep::Text("All done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Do two things").await.unwrap();
        assert_eq!(
            agent.messages().iter().filter(|m| matches!(m, AgentMessage::ToolResult(_))).count(),
            2
        );
    }

    #[tokio::test]
    async fn agent_streaming_callback() {
        let mock = MockProvider::new(vec![MockStep::Text("hello world".into())]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        let received = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let cb = received.clone();
        agent.on_text(move |delta| {
            cb.lock().unwrap().push_str(delta);
        });
        agent.run("Hi").await.unwrap();
        let val = received.lock().unwrap().clone();
        assert!(val.contains("hello"), "Expected 'hello' in stream: {}", val);
    }

    #[tokio::test]
    async fn agent_does_not_execute_tool_calls_from_length_truncated_response() {
        // When the provider returns tool calls with StopReason::Length,
        // the agent should NOT execute those tool calls.
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "should-not-run"}),
                stop_reason: Some(StopReason::Length),
            },
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));
        agent.run("Run echo").await.unwrap();
        let msgs = agent.messages();
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
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "terminator".into(),
                arguments: serde_json::json!({}),
                stop_reason: None,
            },
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(TerminatorTool));
        agent.run("Terminate").await.unwrap();
        let msgs = agent.messages();
        // user + assistant + tool_result = 3 messages (no second round)
        assert_eq!(msgs.len(), 3, "Should stop after terminated tool");
    }

    struct TerminatorTool;

    impl Tool for TerminatorTool {
        fn name(&self) -> &str { "terminator" }
        fn description(&self) -> &str { "Terminates the loop" }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object", "properties": {} })
        }
    }

    #[async_trait]
    impl AgentTool for TerminatorTool {
        fn label(&self) -> &str { "terminator" }

        async fn execute(
            &self,
            _tool_call_id: &str,
            _params: serde_json::Value,
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
        ) -> anyhow::Result<AgentToolResult> {
            Ok(AgentToolResult {
                content: vec![Content::Text { text: "done".into() }],
                terminate: true,
                ..Default::default()
            })
        }
    }
}
