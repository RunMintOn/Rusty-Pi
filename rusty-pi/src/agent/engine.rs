//! Agent — the core loop that drives LLM ↔ tool interactions.
//!
//! Mirrors `@earendil-works/pi-agent-core/src/agent.ts`.
//! Handles the run loop: user prompt → LLM → tool calls → results → LLM → ...

use crate::agent::session::Session;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{
    AgentMessage, AssistantContent, Content, MessageContent, StopReason, TextOrImageContent, Tool,
    ToolResultMessage, UserMessage,
};
use anyhow::{Context, Result};
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

/// The agent that orchestrates the LLM → tool → LLM loop.
pub struct Agent {
    /// Session tree tracking all entries.
    session: Session,
    /// Registered tools.
    tools: Vec<Box<dyn AgentTool>>,
    /// The LLM provider to call.
    provider: Box<dyn ProviderApi>,
    /// Current model.
    model: Model,
    /// Agent configuration.
    config: AgentConfig,
}

impl Agent {
    /// Create a new agent with the given provider and model.
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
        }
    }

    /// Set the agent configuration.
    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a tool to the agent's registry.
    pub fn add_tool(&mut self, tool: Box<dyn AgentTool>) {
        self.tools.push(tool);
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.config.system_prompt = prompt;
    }

    /// Current conversation messages (walked from session tree).
    pub fn messages(&self) -> Vec<&AgentMessage> {
        self.session.messages()
    }

    /// Access the session tree (read-only).
    pub fn session(&self) -> &Session {
        &self.session
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// Collect tool references as `&[&dyn Tool]` for the provider.
    fn tool_refs(&self) -> Vec<&dyn Tool> {
        self.tools.iter().map(|t| t.as_ref() as &dyn Tool).collect()
    }

    /// Run a single user prompt through the agent loop.
    ///
    /// 1. Adds the user message to conversation history.
    /// 2. Calls the LLM provider.
    /// 3. If the LLM returns tool calls, executes them and loops back to step 2.
    /// 4. Returns when the LLM stops (stop/length/error).
    pub async fn run(&mut self, prompt: &str) -> Result<()> {
        let now = Self::now_ms();

        // 1. Add user message
        let user_msg = AgentMessage::User(UserMessage {
            content: MessageContent::Text(prompt.to_string()),
            timestamp: now,
        });
        self.session.add_message(user_msg);

        // 2-3. LLM → tool → LLM loop
        for round in 0..=self.config.max_tool_rounds {
            let tool_refs = self.tool_refs();
            let current_messages: Vec<AgentMessage> = self
                .session
                .messages()
                .into_iter()
                .cloned()
                .collect();

            let response = self
                .provider
                .stream(&self.model, &current_messages, &tool_refs)
                .await
                .with_context(|| format!("LLM call failed at round {}", round))?;

            // Add assistant response to session
            self.session.add_message(AgentMessage::Assistant(
                response.clone(),
            ));

            // 4. Check stop reason
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

                    let tool_calls: Vec<(String, String, serde_json::Value)> = response
                        .content
                        .into_iter()
                        .filter_map(|c| match c {
                            AssistantContent::ToolCall { id, name, arguments } => {
                                Some((id, name, arguments))
                            }
                            _ => None,
                        })
                        .collect();

                    for (tool_call_id, tool_name, args) in &tool_calls {
                        let tool_result = self
                            .execute_tool(tool_call_id, tool_name, args.clone())
                            .await
                            .with_context(|| {
                                format!("Tool '{}' execution failed", tool_name)
                            })?;
                        self.session.add_message(tool_result);
                    }
                }
            }
        }

        anyhow::bail!("Agent loop exited without producing a final response")
    }

    /// Execute a single tool call and return a ToolResultMessage.
    async fn execute_tool(
        &self,
        tool_call_id: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<AgentMessage> {
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

        Ok(AgentMessage::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            content,
            details: Some(serde_json::json!({})),
            is_error: false,
            timestamp: now,
        }))
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
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Echoes the input back"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
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
            _signal: Option<tokio::sync::watch::Receiver<bool>>,
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
                assert!(a
                    .content
                    .iter()
                    .any(|c| matches!(c, AssistantContent::Text { .. })));
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
                arguments: serde_json::json!({"text": "hello world"}),
            },
            MockStep::Text("Done!".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));

        agent.run("Run echo").await.unwrap();
        let msgs = agent.messages();

        assert!(
            msgs.len() >= 4,
            "Expected at least 4 messages, got {}",
            msgs.len()
        );

        let tool_result = &msgs[msgs.len() - 2];
        match tool_result {
            AgentMessage::ToolResult(tr) => {
                assert_eq!(tr.tool_name, "echo");
            }
            _ => panic!("Expected tool result message"),
        }

        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected assistant message"),
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
            _ => panic!("Expected assistant message"),
        }
    }

    #[tokio::test]
    async fn agent_with_bash_tool() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_bash".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "echo bash-works"}),
            },
            MockStep::Text("Done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(
            crate::coding_agent::tools::bash::BashTool::new(
                std::env::current_dir()
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            ),
        ));

        agent.run("Run bash").await.unwrap();
        let msgs = agent.messages();

        assert!(
            msgs.len() >= 4,
            "Expected at least 4 messages, got {}",
            msgs.len()
        );

        let tool_result = &msgs[msgs.len() - 2];
        match tool_result {
            AgentMessage::ToolResult(tr) => {
                assert_eq!(tr.tool_name, "bash");
            }
            _ => panic!("Expected tool result message"),
        }

        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected assistant message"),
        }
    }

    #[tokio::test]
    async fn agent_multiple_rounds() {
        let mock = MockProvider::new(vec![
            MockStep::ToolCall {
                id: "tc_1".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "first"}),
            },
            MockStep::ToolCall {
                id: "tc_2".into(),
                name: "echo".into(),
                arguments: serde_json::json!({"text": "second"}),
            },
            MockStep::Text("All done".into()),
        ]);
        let mut agent = Agent::new(Box::new(mock), make_model());
        agent.add_tool(Box::new(EchoTool));

        agent.run("Do two things").await.unwrap();
        let msgs = agent.messages();
        let last = msgs.last().unwrap();
        match last {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.stop_reason, StopReason::Stop);
            }
            _ => panic!("Expected assistant message"),
        }
        let tool_result_count = msgs
            .iter()
            .filter(|m| matches!(m, AgentMessage::ToolResult(_)))
            .count();
        assert_eq!(tool_result_count, 2, "Expected 2 tool result messages");
    }
}
