//! Agent — the core loop that drives LLM ↔ tool interactions.

use crate::agent::session::Session;
use crate::agent::types::AgentTool;
use crate::ai::providers::{Model, ProviderApi};
use crate::ai::stream::StreamEvent;
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

/// Callback for streaming text deltas.
pub type TextCallback = Box<dyn FnMut(&str) + Send>;

/// The agent that orchestrates the LLM → tool → LLM loop.
pub struct Agent {
    session: Session,
    tools: Vec<Box<dyn AgentTool>>,
    provider: Box<dyn ProviderApi>,
    model: Model,
    config: AgentConfig,
    /// Optional callback for streaming text deltas.
    on_text: Option<TextCallback>,
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
        }
    }

    pub fn with_config(mut self, config: AgentConfig) -> Self {
        self.config = config;
        self
    }

    /// Register a callback for streaming text deltas.
    pub fn on_text<F>(&mut self, callback: F)
    where
        F: FnMut(&str) + Send + 'static,
    {
        self.on_text = Some(Box::new(callback));
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

            // Collect stream events into an AssistantMessage
            let mut content_buf: Vec<(String, String, serde_json::Value)> = Vec::new();
            let mut text_buf = String::new();
            let mut stop_reason = StopReason::Stop;
            let mut error_msg = None;

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::TextDelta { delta } => {
                        text_buf.push_str(&delta);
                        if let Some(ref mut cb) = self.on_text {
                            cb(&delta);
                        }
                    }
                    StreamEvent::ToolCall { id, name, arguments } => {
                        content_buf.push((id, name, arguments));
                    }
                    StreamEvent::Done { message } => {
                        stop_reason = message.stop_reason;
                        error_msg = message.error_message;
                        break;
                    }
                    StreamEvent::Error { reason, message } => {
                        stop_reason = reason;
                        error_msg = Some(message);
                        break;
                    }
                }
            }

            // Build assistant content
            let mut content: Vec<AssistantContent> = Vec::new();
            if !text_buf.is_empty() {
                content.push(AssistantContent::Text { text: text_buf });
            }
            for (id, name, arguments) in &content_buf {
                content.push(AssistantContent::ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments: arguments.clone(),
                });
            }

            let response = crate::ai::types::AssistantMessage {
                content,
                api: self.model.api.to_string(),
                provider: self.model.id.to_string(),
                model: self.model.id.to_string(),
                usage: None,
                stop_reason,
                error_message: error_msg,
                timestamp: Self::now_ms(),
            };

            // Determine if we need to execute tools
            let has_tool_calls = !content_buf.is_empty();
            let stop_reason = if has_tool_calls { StopReason::ToolUse } else { response.stop_reason };

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

                    for (tool_call_id, tool_name, args) in &content_buf {
                        let tool_result = self
                            .execute_tool(tool_call_id, tool_name, args.clone())
                            .await
                            .with_context(|| format!("Tool '{}' execution failed", tool_name))?;
                        self.session.add_message(tool_result);
                    }
                }
            }
        }

        anyhow::bail!("Agent loop exited without producing a final response")
    }

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
            MockStep::ToolCall { id: "tc_1".into(), name: "echo".into(), arguments: serde_json::json!({"text": "hello"}) },
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
            MockStep::ToolCall { id: "tc_bash".into(), name: "bash".into(), arguments: serde_json::json!({"command": "echo bash-works"}) },
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
            MockStep::ToolCall { id: "tc_1".into(), name: "echo".into(), arguments: serde_json::json!({"text": "first"}) },
            MockStep::ToolCall { id: "tc_2".into(), name: "echo".into(), arguments: serde_json::json!({"text": "second"}) },
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
}
