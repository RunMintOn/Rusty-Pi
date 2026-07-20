//! DeepSeek LLM provider — OpenAI-compatible completions API.
//!
//! Also works with any OpenAI-compatible endpoint by setting `DEEPSEEK_BASE_URL`
//! (defaults to `https://api.deepseek.com`).
//!
//! # Authentication
//!
//! Reads the API key from the `DEEPSEEK_API_KEY` environment variable.
//! For local / self-hosted endpoints, set both:
//! ```ignore
//! export DEEPSEEK_API_KEY="local-free-models"
//! export DEEPSEEK_BASE_URL="http://127.0.0.1:18180/v1"
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use rusty_pi::ai::providers::deepseek::DeepSeekProvider;
//! use rusty_pi::ai::providers::ProviderApi;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let provider = DeepSeekProvider::from_env().expect("DEEPSEEK_API_KEY not set");
//! # Ok(())
//! # }
//! ```
//!
//! # Streaming
//!
//! Uses OpenAI-compatible SSE streaming (`data:` lines) to emit tokens as they arrive.
//! Tool calls are parsed from the `tool_calls` delta field in each SSE chunk.
//! Tool call arguments are accumulated across multiple chunks before being parsed.

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;
use std::collections::HashMap;

const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";

/// Models supported by the DeepSeek provider.
///
/// Add extra models at runtime by setting the `DEEPSEEK_MODEL_ID` env var.
pub const DEEPSEEK_MODELS: &[Model] = &[
    Model {
        id: "deepseek-v4-flash",
        api: "openai-completions",
    },
    Model {
        id: "deepseek-v4-pro",
        api: "openai-completions",
    },
    Model {
        id: "deepseek-v4-flash-free",
        api: "openai-completions",
    },
];

/// Provider for the DeepSeek API (OpenAI-compatible chat completions endpoint).
///
/// Reads the API key from `DEEPSEEK_API_KEY` at construction time.
/// Supports streaming SSE responses and tool calls via the standard
/// OpenAI chat completions wire format.
pub struct DeepSeekProvider {
    api_key: String,
    base_url: String,
}

impl DeepSeekProvider {
    /// Create a provider from environment variables.
    ///
    /// Reads:
    /// - `DEEPSEEK_API_KEY` (required) — API key
    /// - `DEEPSEEK_BASE_URL` (optional) — base URL, defaults to `https://api.deepseek.com`
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(DEEPSEEK_API_KEY_ENV).ok()?;
        let base_url = std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEEPSEEK_BASE_URL.to_string());
        Some(Self::new(api_key).with_base_url(base_url))
    }

    /// Create a new DeepSeek provider with the given API key.
    ///
    /// Uses the default base URL (`https://api.deepseek.com`).
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEEPSEEK_BASE_URL.to_string(),
        }
    }

    /// Override the base URL (for proxies or self-hosted endpoints).
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// Convert agent messages to the OpenAI chat completions wire format.
    ///
    /// Handles `role: "user"`, `role: "assistant"` (with optional `tool_calls`),
    /// and `role: "tool"` messages.
    /// The first message can be a system prompt (role: "system") if provided.
    pub(crate) fn build_messages(messages: &[AgentMessage], system_prompt: Option<&str>) -> Vec<serde_json::Value> {
        let mut result = Vec::new();

        // Prepend system message if provided
        if let Some(prompt) = system_prompt
            && !prompt.is_empty()
        {
            result.push(serde_json::json!({
                "role": "system",
                "content": prompt
            }));
        }

        for msg in messages {
            match msg {
                AgentMessage::User(u) => {
                    let content = match &u.content {
                        crate::ai::types::MessageContent::Text(t) => serde_json::Value::String(t.clone()),
                        crate::ai::types::MessageContent::Blocks(blocks) => {
                            serde_json::to_value(blocks).unwrap_or_default()
                        }
                    };
                    result.push(serde_json::json!({ "role": "user", "content": content }));
                }
                AgentMessage::Assistant(a) => {
                    let text: String = a
                        .content
                        .iter()
                        .filter_map(|c| {
                            if let AssistantContent::Text { text } = c {
                                Some(text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    // Build tool_calls array from assistant content
                    let tool_calls: Vec<serde_json::Value> = a
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            AssistantContent::ToolCall { id, name, arguments } => Some(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(arguments).unwrap_or_default()
                                }
                            })),
                            _ => None,
                        })
                        .collect();

                    // Assistant messages must have either content or tool_calls
                    // (or both). Empty-text-only messages are skipped.
                    if text.is_empty() && tool_calls.is_empty() {
                        continue;
                    }

                    let mut msg = serde_json::json!({
                        "role": "assistant",
                        "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(text) }
                    });
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = serde_json::Value::Array(tool_calls);
                    }
                    result.push(msg);
                }
                AgentMessage::ToolResult(tr) => {
                    let content = tr
                        .content
                        .iter()
                        .filter_map(|c| {
                            if let crate::ai::types::TextOrImageContent::Text { text } = c {
                                Some(text.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_call_id,
                        "content": content
                    }));
                }
                // Synthetic context-only messages are never sent to the LLM API.
                _ => {}
            }
        }

        result
    }

    /// Convert Tool trait objects to OpenAI tool definition JSON.
    fn build_tools(tools: &[&dyn Tool]) -> Vec<serde_json::Value> {
        tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name(),
                        "description": t.description(),
                        "parameters": t.parameters()
                    }
                })
            })
            .collect()
    }
}

#[async_trait]
impl ProviderApi for DeepSeekProvider {
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        tools: &[&dyn Tool],
        system_prompt: Option<&str>,
    ) -> anyhow::Result<StreamReceiver> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let url = format!("{}/chat/completions", self.base_url);
        let api_key = self.api_key.clone();
        let api_messages = Self::build_messages(messages, system_prompt);
        let api_tools = Self::build_tools(tools);
        let model_id = model.id.to_string();

        tokio::spawn(async move {
            if let Err(e) = do_stream(&url, &api_key, &model_id, &api_messages, &api_tools, tx).await {
                eprintln!("[deepseek] {}", e);
            }
        });

        Ok(rx)
    }

    fn list_models(&self) -> Vec<&Model> {
        DEEPSEEK_MODELS.iter().collect()
    }
}

/// Build the request body JSON for the DeepSeek API.
fn build_request_body(
    model_id: &str,
    messages: &[serde_json::Value],
    tools: &[serde_json::Value],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model_id,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true }
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools.to_vec());
    }
    body
}

/// State for accumulating tool call arguments across streaming chunks.
struct ToolCallAccumulator {
    /// Key: tool_call index (from delta), Value: (id, name, accumulated arguments string).
    calls: HashMap<i64, (String, String, String)>,
}

impl ToolCallAccumulator {
    fn new() -> Self {
        Self { calls: HashMap::new() }
    }

    /// Process a single tool_call delta from the stream.
    fn push_delta(&mut self, call: &serde_json::Value) {
        let index = call["index"].as_i64().unwrap_or(0);
        let entry = self.calls.entry(index).or_insert_with(|| {
            let id = call["id"].as_str().unwrap_or("call_unknown").to_string();
            let name = call["function"]["name"].as_str().unwrap_or("unknown").to_string();
            (id, name, String::new())
        });

        // Accumulate argument fragments
        if let Some(args_delta) = call["function"]["arguments"].as_str() {
            entry.2.push_str(args_delta);
        }
    }

    /// Finalize all accumulated tool calls, parsing their JSON arguments.
    /// Returns (tool_call_id, name, parsed_arguments) for each call.
    /// Consumes the internal state so subsequent calls return empty.
    fn finalize(&mut self) -> Vec<(String, String, serde_json::Value)> {
        std::mem::take(&mut self.calls)
            .into_iter()
            .map(|(_index, (id, name, args_str))| {
                let arguments = if args_str.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}))
                };
                (id, name, arguments)
            })
            .collect()
    }
}

async fn do_stream(
    url: &str,
    api_key: &str,
    model_id: &str,
    messages: &[serde_json::Value],
    tools: &[serde_json::Value],
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
) -> anyhow::Result<()> {
    let http_client = Client::new();
    let body = build_request_body(model_id, messages, tools);

    let response = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_body: serde_json::Value = response.json().await.unwrap_or_default();
        let error_msg = error_body["error"]["message"].as_str().unwrap_or("Unknown API error");
        let err = format!("DeepSeek API error ({}): {}", status.as_u16(), error_msg);
        let _ = tx
            .send(StreamEvent::Error {
                reason: StopReason::Error,
                message: err.clone(),
            })
            .await;
        anyhow::bail!("{}", err);
    }

    let mut acc = MessageAccumulator::new("openai-completions", "deepseek", model_id);
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut tool_accum = ToolCallAccumulator::new();
    let mut has_tool_calls = false;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();
            if line.is_empty() || line.starts_with(':') || line == "data: [DONE]" {
                continue;
            }

            if let Some(json_str) = line.strip_prefix("data: ") {
                match serde_json::from_str::<serde_json::Value>(json_str) {
                    Ok(data) => {
                        if let Some(choices) = data["choices"].as_array() {
                            for choice in choices {
                                let delta = &choice["delta"];
                                let finish_reason = choice["finish_reason"].as_str();

                                // Text content delta
                                if let Some(content) = delta["content"].as_str()
                                    && !content.is_empty()
                                {
                                    let _ = tx
                                        .send(StreamEvent::TextDelta {
                                            delta: content.to_string(),
                                        })
                                        .await;
                                }

                                // Tool call delta — accumulate arguments across chunks
                                if let Some(tc_array) = delta["tool_calls"].as_array() {
                                    for call in tc_array {
                                        has_tool_calls = true;
                                        tool_accum.push_delta(call);
                                    }
                                }

                                // Finish reason — finalize tool calls
                                if let Some(reason) = finish_reason
                                    && !reason.is_empty()
                                    && reason != "null"
                                {
                                    let stop = match reason {
                                        "stop" => StopReason::Stop,
                                        "length" => StopReason::Length,
                                        "tool_calls" => StopReason::ToolUse,
                                        _ => StopReason::Stop,
                                    };

                                    // Emit accumulated tool calls
                                    for (id, name, arguments) in tool_accum.finalize() {
                                        let _ = tx.send(StreamEvent::ToolCall { id, name, arguments }).await;
                                    }

                                    acc.push(StreamEvent::Done {
                                        message: AssistantMessage {
                                            content: vec![],
                                            api: "openai-completions".into(),
                                            provider: "deepseek".into(),
                                            model: model_id.to_string(),
                                            usage: None,
                                            stop_reason: stop,
                                            error_message: None,
                                            timestamp: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default()
                                                .as_millis()
                                                as i64,
                                        },
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(StreamEvent::Error {
                                reason: StopReason::Error,
                                message: format!("SSE parse error: {}", e),
                            })
                            .await;
                        anyhow::bail!("SSE parse error: {}", e);
                    }
                }
            }
        }
    }

    // Stream ended — emit any remaining tool calls that weren't emitted
    // (edge case: tool_calls arrived without a finish_reason)
    let remaining = tool_accum.finalize();
    if !remaining.is_empty() && !has_tool_calls {
        // Shouldn't normally happen, but be safe
        for (id, name, arguments) in remaining {
            let _ = tx.send(StreamEvent::ToolCall { id, name, arguments }).await;
        }
    }

    let msg = acc.build();
    let _ = tx.send(StreamEvent::Done { message: msg }).await;
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{
        AgentMessage, AssistantContent, AssistantMessage, MessageContent, StopReason, TextOrImageContent,
        ToolResultMessage, UserMessage,
    };

    // ── build_messages tests ────────────────────────────────────────────────

    #[test]
    fn build_messages_includes_system_prompt() {
        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hello".into()),
            timestamp: 1000,
        })];
        let result = DeepSeekProvider::build_messages(&messages, Some("You are helpful"));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0]["role"], "system");
        assert_eq!(result[0]["content"], "You are helpful");
        assert_eq!(result[1]["role"], "user");
    }

    #[test]
    fn build_messages_no_system_prompt_when_none() {
        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hello".into()),
            timestamp: 1000,
        })];
        let result = DeepSeekProvider::build_messages(&messages, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
    }

    #[test]
    fn build_messages_empty_system_prompt_skipped() {
        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hello".into()),
            timestamp: 1000,
        })];
        let result = DeepSeekProvider::build_messages(&messages, Some(""));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
    }

    #[test]
    fn build_messages_preserves_assistant_tool_call() {
        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("Run ls".into()),
                timestamp: 1000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    AssistantContent::Text {
                        text: "I'll run that".into(),
                    },
                    AssistantContent::ToolCall {
                        id: "call_123".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "ls"}),
                    },
                ],
                api: "openai-completions".into(),
                provider: "deepseek".into(),
                model: "deepseek-v4-pro".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2000,
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "call_123".into(),
                tool_name: "bash".into(),
                content: vec![TextOrImageContent::Text {
                    text: "src\nCargo.toml".into(),
                }],
                details: None,
                is_error: false,
                timestamp: 3000,
            }),
        ];
        let result = DeepSeekProvider::build_messages(&messages, None);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[1]["role"], "assistant");
        assert!(result[1]["tool_calls"].is_array());
        assert_eq!(result[1]["tool_calls"][0]["id"], "call_123");
        assert_eq!(result[1]["tool_calls"][0]["function"]["name"], "bash");
        assert_eq!(result[2]["role"], "tool");
        assert_eq!(result[2]["tool_call_id"], "call_123");
    }

    #[test]
    fn build_messages_skips_empty_text_assistant_with_tool_calls() {
        // Assistant message with only tool calls (no text) must be preserved
        let messages = vec![AgentMessage::Assistant(AssistantMessage {
            content: vec![AssistantContent::ToolCall {
                id: "call_456".into(),
                name: "read".into(),
                arguments: serde_json::json!({"path": "/tmp/f"}),
            }],
            api: "openai-completions".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            usage: None,
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 2000,
        })];
        let result = DeepSeekProvider::build_messages(&messages, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        assert!(result[0]["content"].is_null());
        assert_eq!(result[0]["tool_calls"][0]["id"], "call_456");
    }

    #[test]
    fn build_messages_skips_empty_text_assistant_without_tool_calls() {
        // Assistant with empty content and no tool calls should be skipped
        let messages = vec![AgentMessage::Assistant(AssistantMessage {
            content: vec![],
            api: "openai-completions".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            usage: None,
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2000,
        })];
        let result = DeepSeekProvider::build_messages(&messages, None);
        assert!(result.is_empty());
    }

    // ── build_tools tests ──────────────────────────────────────────────────

    struct TestTool;

    impl Tool for TestTool {
        fn name(&self) -> &str {
            "bash"
        }
        fn description(&self) -> &str {
            "Execute bash commands"
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command to run" }
                },
                "required": ["command"]
            })
        }
    }

    #[test]
    fn build_tools_serializes_correctly() {
        let tools: Vec<&dyn Tool> = vec![&TestTool];
        let result = DeepSeekProvider::build_tools(&tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["type"], "function");
        assert_eq!(result[0]["function"]["name"], "bash");
        assert_eq!(result[0]["function"]["description"], "Execute bash commands");
        assert_eq!(
            result[0]["function"]["parameters"]["properties"]["command"]["type"],
            "string"
        );
    }

    #[test]
    fn build_tools_empty_list() {
        let tools: Vec<&dyn Tool> = vec![];
        let result = DeepSeekProvider::build_tools(&tools);
        assert!(result.is_empty());
    }

    // ── build_request_body tests ──────────────────────────────────────────

    #[test]
    fn request_body_includes_tools_when_present() {
        let body = build_request_body(
            "deepseek-v4-pro",
            &[serde_json::json!({"role": "user", "content": "hi"})],
            &[serde_json::json!({
                "type": "function",
                "function": { "name": "bash", "description": "Run", "parameters": {} }
            })],
        );
        assert_eq!(body["model"], "deepseek-v4-pro");
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["function"]["name"], "bash");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn request_body_omits_tools_when_empty() {
        let body = build_request_body(
            "deepseek-v4-flash",
            &[serde_json::json!({"role": "user", "content": "hi"})],
            &[],
        );
        assert!(!body.as_object().unwrap().contains_key("tools"));
    }

    // ── ToolCallAccumulator tests ─────────────────────────────────────────

    #[test]
    fn accumulator_single_chunk() {
        let mut acc = ToolCallAccumulator::new();
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_1",
            "function": { "name": "bash", "arguments": "{\"command\":\"ls\"}" }
        }));
        let result = acc.finalize();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "call_1");
        assert_eq!(result[0].1, "bash");
        assert_eq!(result[0].2["command"], "ls");
    }

    #[test]
    fn accumulator_multi_chunk_arguments() {
        let mut acc = ToolCallAccumulator::new();
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_1",
            "function": { "name": "bash", "arguments": "{\"path\":" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": { "arguments": "\"/tmp/a\"" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": { "arguments": "}" }
        }));
        let result = acc.finalize();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].2["path"], "/tmp/a");
    }

    #[test]
    fn accumulator_multiple_parallel_tool_calls() {
        let mut acc = ToolCallAccumulator::new();
        // Two tool calls interleaved
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_a",
            "function": { "name": "read", "arguments": "{\"path\":" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 1,
            "id": "call_b",
            "function": { "name": "bash", "arguments": "{\"c" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": { "arguments": "\"/tmp/x\"}" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 1,
            "function": { "arguments": "ommand\":\"echo hi\"}" }
        }));
        let result = acc.finalize();
        assert_eq!(result.len(), 2);
        let call_a = result.iter().find(|(id, _, _)| id == "call_a").unwrap();
        let call_b = result.iter().find(|(id, _, _)| id == "call_b").unwrap();
        assert_eq!(call_a.2["path"], "/tmp/x");
        assert_eq!(call_b.2["command"], "echo hi");
    }

    #[test]
    fn accumulator_incomplete_json_uses_empty_object() {
        let mut acc = ToolCallAccumulator::new();
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_bad",
            "function": { "name": "bash", "arguments": "{\"bad" }
        }));
        let result = acc.finalize();
        assert_eq!(result.len(), 1);
        // Broken JSON should produce {} as fallback
        assert!(result[0].2.is_object());
    }

    #[test]
    fn accumulator_no_id_reuses_previous() {
        let mut acc = ToolCallAccumulator::new();
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_real",
            "function": { "name": "read", "arguments": "" }
        }));
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": { "name": "read", "arguments": "{\"p\":1}" }
        }));
        let result = acc.finalize();
        assert_eq!(result[0].0, "call_real");
        assert_eq!(result[0].2["p"], 1);
    }

    // ── Error handling tests using wiremock ──────────────────────────────────

    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn http_500_returns_error_stop_reason() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "error": { "message": "Internal server error" }
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let provider = DeepSeekProvider::new("test-key".into()).with_base_url(mock_server.uri());

        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hi".into()),
            timestamp: 1000,
        })];

        let mut rx = provider
            .stream(
                &Model {
                    id: "deepseek-v4-flash",
                    api: "openai-completions",
                },
                &messages,
                &[],
                None,
            )
            .await
            .unwrap();

        // Should receive an Error event, not a normal Done with Stop
        let mut got_error = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { reason, message } => {
                    assert_eq!(reason, StopReason::Error);
                    assert!(message.contains("500"));
                    got_error = true;
                }
                StreamEvent::Done { message } => {
                    // If Done arrives, it should also indicate an error
                    assert_eq!(message.stop_reason, StopReason::Error);
                    got_error = true;
                }
                _ => {}
            }
        }
        assert!(got_error, "Should have received an error event");
    }

    #[tokio::test]
    async fn invalid_json_in_sse_emits_error() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: {invalid json\n\n"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let provider = DeepSeekProvider::new("test-key".into()).with_base_url(mock_server.uri());

        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hi".into()),
            timestamp: 1000,
        })];

        let mut rx = provider
            .stream(
                &Model {
                    id: "deepseek-v4-flash",
                    api: "openai-completions",
                },
                &messages,
                &[],
                None,
            )
            .await
            .unwrap();

        let mut got_error = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Error { reason, message } => {
                    assert_eq!(reason, StopReason::Error);
                    assert!(message.contains("SSE parse error") || message.contains("error"));
                    got_error = true;
                }
                StreamEvent::Done { message } => {
                    assert_eq!(message.stop_reason, StopReason::Error);
                    got_error = true;
                }
                _ => {}
            }
        }
        assert!(got_error, "Invalid JSON should produce an error event");
    }

    #[tokio::test]
    async fn stream_disconnect_mid_response_is_handled() {
        let mock_server = MockServer::start().await;
        // Return a valid SSE stream that stops abruptly (no finish_reason)
        let sse_body = "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string(sse_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let provider = DeepSeekProvider::new("test-key".into()).with_base_url(mock_server.uri());

        let messages = vec![AgentMessage::User(UserMessage {
            content: MessageContent::Text("hi".into()),
            timestamp: 1000,
        })];

        let mut rx = provider
            .stream(
                &Model {
                    id: "deepseek-v4-flash",
                    api: "openai-completions",
                },
                &messages,
                &[],
                None,
            )
            .await
            .unwrap();

        // Should get at least a TextDelta and a Done event
        let mut got_text = false;
        let mut got_done = false;
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::TextDelta { delta } => {
                    assert!(delta.contains("hello"));
                    got_text = true;
                }
                StreamEvent::Done { message } => {
                    // When stream ends without finish_reason, stop defaults to Stop
                    assert_eq!(message.stop_reason, StopReason::Stop);
                    got_done = true;
                }
                _ => {}
            }
        }
        assert!(got_text, "Should have received text delta");
        assert!(got_done, "Should have received Done event");
    }

    #[tokio::test]
    async fn multi_chunk_tool_call_arguments_are_accumulated() {
        // Test the tool call accumulator directly with properly formatted chunks
        let mut acc = ToolCallAccumulator::new();

        // Chunk 1: id + name + partial args
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "id": "call_abc",
            "function": {
                "name": "bash",
                "arguments": "{\"comm"
            }
        }));

        // Chunk 2: more args
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": {
                "arguments": "and\":\"echo hi"
            }
        }));

        // Chunk 3: final args
        acc.push_delta(&serde_json::json!({
            "index": 0,
            "function": {
                "arguments": "\"}"
            }
        }));

        // Finalize and check
        let result = acc.finalize();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "call_abc");
        assert_eq!(result[0].1, "bash");
        // The accumulated arguments should be: {"command":"echo hi"}
        assert_eq!(result[0].2["command"], "echo hi");
    }
}
