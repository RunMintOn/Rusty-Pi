//! OpenAI Codex provider — ChatGPT Plus/Pro API.
//!
//! Communicates with the `chatgpt.com/backend-api` endpoint using the
//! OpenAI Responses API wire format. Supports both SSE (HTTP streaming)
//! and WebSocket transports in the original implementation; the Rust port
//! currently uses HTTP with full-response reading (SSE streaming planned
//! in ticket 06).
//!
//! # Authentication
//!
//! Requires a ChatGPT Plus or Pro subscription. There are two auth paths:
//!
//! 1. **Manual token** (implemented): Set `OPENAI_CODEX_TOKEN` to a raw
//!    JWT obtained from the ChatGPT web app. This is a development convenience.
//! 2. **OAuth flow** (future): The original implementation supports a full
//!    OAuth 2.0 device-code flow that obtains and refreshes tokens
//!    automatically. See `reference/earendil-works-pi/packages/ai/src/auth/oauth/openai-codex.ts`.
//!
//! # Wire format
//!
//! Uses the OpenAI Responses API (`/responses` endpoint) with typed input
//! items: `message`, `function_call`, `function_call_output`. Tool calls
//! from previous turns are passed as `function_call` items in the request
//! input array.

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::StreamEvent;
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;

const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const CODEX_TOKEN_ENV: &str = "OPENAI_CODEX_TOKEN";

/// Models supported by the OpenAI Codex provider.
pub const OPENAI_CODEX_MODELS: &[Model] = &[
    Model { id: "gpt-5.6-sol", api: "openai-codex-responses" },
    Model { id: "gpt-5.6-luna", api: "openai-codex-responses" },
    Model { id: "gpt-5.5", api: "openai-codex-responses" },
    Model { id: "gpt-5.4", api: "openai-codex-responses" },
    Model { id: "gpt-5.4-mini", api: "openai-codex-responses" },
];

/// Provider for the OpenAI Codex API (ChatGPT Plus/Pro).
///
/// Uses the `chatgpt.com/backend-api/responses` endpoint with the OpenAI
/// Responses API wire format. Requires a JWT access token obtained from
/// a ChatGPT Plus or Pro subscription.
///
/// # Auth
///
/// The token is read from `OPENAI_CODEX_TOKEN` at construction time.
/// The original implementation also supports an OAuth device-code flow;
/// see ticket 07 for the planned port.
pub struct OpenAICodexProvider {
    token: String,
}

impl OpenAICodexProvider {
    /// Create a provider from the `OPENAI_CODEX_TOKEN` environment variable.
    /// Returns `None` if the variable is not set.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var(CODEX_TOKEN_ENV).ok()?;
        Some(Self::new(token))
    }

    /// Create a new Codex provider with the given JWT access token.
    pub fn new(token: String) -> Self {
        Self { token }
    }

    fn build_input(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
        let mut items = Vec::new();
        for msg in messages {
            match msg {
                AgentMessage::User(u) => {
                    let text = match &u.content {
                        crate::ai::types::MessageContent::Text(t) => t.clone(),
                        _ => "[content]".to_string(),
                    };
                    items.push(serde_json::json!({
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": text}]
                    }));
                }
                AgentMessage::Assistant(a) => {
                    let text: String = a.content.iter()
                        .filter_map(|c| if let AssistantContent::Text { text } = c { Some(text.as_str()) } else { None })
                        .collect::<Vec<_>>().join("\n");
                    if !text.is_empty() {
                        items.push(serde_json::json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": text}]
                        }));
                    }
                    for c in &a.content {
                        if let AssistantContent::ToolCall { id, name, arguments } = c {
                            let parts: Vec<&str> = id.split('|').collect();
                            let call_id = parts.first().copied().unwrap_or(id.as_str());
                            let item_id = parts.get(1).copied().unwrap_or("");
                            items.push(serde_json::json!({
                                "type": "function_call",
                                "id": item_id,
                                "call_id": call_id,
                                "name": name,
                                "arguments": serde_json::to_string(arguments).unwrap_or_default()
                            }));
                        }
                    }
                }
                AgentMessage::ToolResult(tr) => {
                    let content = tr.content.iter()
                        .filter_map(|c| if let crate::ai::types::TextOrImageContent::Text { text } = c { Some(text.clone()) } else { None })
                        .collect::<Vec<_>>().join("\n");
                    let call_id = tr.tool_call_id.split('|').next().unwrap_or(&tr.tool_call_id);
                    items.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": content
                    }));
                }
                // Synthetic context-only messages (BranchSummary, CompactionSummary, CustomContext)
                // are never sent to the LLM API — skip silently.
                _ => {}
            }
        }
        items
    }
}

#[async_trait]
impl ProviderApi for OpenAICodexProvider {
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<StreamReceiver> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let url = format!("{}/responses", CODEX_BASE_URL);
        let token = self.token.clone();
        let input = Self::build_input(messages);
        let model_id = model.id.to_string();

        tokio::spawn(async move {
            if let Err(e) = do_codex_stream(&url, &token, &model_id, &input, tx).await {
                eprintln!("[codex] {}", e);
            }
        });

        Ok(rx)
    }
}

async fn do_codex_stream(
    url: &str, token: &str, model_id: &str,
    input: &[serde_json::Value], tx: tokio::sync::mpsc::Sender<StreamEvent>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "model": model_id, "input": input });

    let response = client.post(url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("OpenAI-Beta", "responses=v1")
        .json(&body).send().await?;

    let status = response.status();
    let response_body: serde_json::Value = response.json().await?;
    if !status.is_success() {
        let error_msg = response_body["error"]["message"].as_str().unwrap_or("Unknown Codex error");
        anyhow::bail!("Codex API error ({}): {}", status.as_u16(), error_msg);
    }

    let output = &response_body["output"];
    let mut text_parts = Vec::new();
    let mut has_tool_calls = false;

    if let Some(items) = output.as_array() {
        for item in items {
            match item["type"].as_str() {
                Some("message") => {
                    if let Some(parts) = item["content"].as_array() {
                        for part in parts {
                            if let Some("output_text") = part["type"].as_str()
                                && let Some(text) = part["text"].as_str()
                                    && !text.is_empty() {
                                        let _ = tx.send(StreamEvent::TextDelta { delta: text.to_string() }).await;
                                        text_parts.push(text.to_string());
                                    }
                        }
                    }
                }
                Some("function_call") => {
                    has_tool_calls = true;
                    let call_id = item["call_id"].as_str().unwrap_or("call_unknown");
                    let item_id = item["id"].as_str().unwrap_or("");
                    let composite_id = if item_id.is_empty() {
                        call_id.to_string()
                    } else {
                        format!("{}|{}", call_id, item_id)
                    };
                    let name = item["name"].as_str().unwrap_or("unknown");
                    let args_str = item["arguments"].as_str().unwrap_or("{}");
                    if let Ok(arguments) = serde_json::from_str(args_str) {
                        let _ = tx.send(StreamEvent::ToolCall {
                            id: composite_id, name: name.to_string(), arguments,
                        }).await;
                    }
                }
                _ => {}
            }
        }
    }

    // Derive stop reason from response status
    let response_status = response_body["status"].as_str().unwrap_or("completed");
    let stop_reason = match response_status {
        "incomplete" => StopReason::Length,
        "failed" | "cancelled" => StopReason::Error,
        _ => {
            if has_tool_calls {
                StopReason::ToolUse
            } else {
                StopReason::Stop
            }
        }
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;

    let msg = AssistantMessage {
        content: vec![],
        api: "openai-codex-responses".into(), provider: "openai-codex".into(),
        model: model_id.to_string(), usage: None,
        stop_reason, error_message: None, timestamp: now,
    };
    let _ = tx.send(StreamEvent::Done { message: msg }).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, MessageContent, StopReason, TextOrImageContent, ToolResultMessage, UserMessage};

    #[test]
    fn build_input_includes_function_call_items_for_tool_calls() {
        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("Run ls".into()),
                timestamp: 1000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    AssistantContent::Text { text: "I'll run that".into() },
                    AssistantContent::ToolCall {
                        id: "call_abc|fc_item_1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "ls"}),
                    },
                ],
                api: "openai-codex-responses".into(),
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 2000,
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "call_abc|fc_item_1".into(),
                tool_name: "bash".into(),
                content: vec![TextOrImageContent::Text { text: "src\nCargo.toml".into() }],
                details: None,
                is_error: false,
                timestamp: 3000,
            }),
        ];

        let input = OpenAICodexProvider::build_input(&messages);
        assert_eq!(input.len(), 4, "should produce 4 items: user msg, assistant msg, function_call, function_call_output");

        // User message
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[0]["role"], "user");

        // Assistant message
        assert_eq!(input[1]["type"], "message");
        assert_eq!(input[1]["role"], "assistant");

        // Function call (tool call from assistant)
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(input[2]["call_id"], "call_abc");
        assert_eq!(input[2]["id"], "fc_item_1");
        assert_eq!(input[2]["name"], "bash");

        // Function call output (tool result)
        assert_eq!(input[3]["type"], "function_call_output");
        assert_eq!(input[3]["call_id"], "call_abc");
    }

    #[test]
    fn build_input_includes_only_text_when_no_tool_calls() {
        let messages = vec![
            AgentMessage::User(UserMessage {
                content: MessageContent::Text("Hello".into()),
                timestamp: 1000,
            }),
            AgentMessage::Assistant(AssistantMessage {
                content: vec![AssistantContent::Text { text: "Hi there".into() }],
                api: "openai-codex-responses".into(),
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                usage: None,
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 2000,
            }),
        ];

        let input = OpenAICodexProvider::build_input(&messages);
        assert_eq!(input.len(), 2);
        // Both should be "message" type, no function_call items
        assert_eq!(input[0]["type"], "message");
        assert_eq!(input[1]["type"], "message");
    }

    #[test]
    fn build_input_handles_tool_call_id_without_pipe() {
        let messages = vec![
            AgentMessage::Assistant(AssistantMessage {
                content: vec![
                    AssistantContent::ToolCall {
                        id: "simple_call_id".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({}),
                    },
                ],
                api: "openai-codex-responses".into(),
                provider: "openai-codex".into(),
                model: "gpt-5.5".into(),
                usage: None,
                stop_reason: StopReason::ToolUse,
                error_message: None,
                timestamp: 1000,
            }),
            AgentMessage::ToolResult(ToolResultMessage {
                tool_call_id: "simple_call_id".into(),
                tool_name: "bash".into(),
                content: vec![TextOrImageContent::Text { text: "ok".into() }],
                details: None,
                is_error: false,
                timestamp: 2000,
            }),
        ];

        let input = OpenAICodexProvider::build_input(&messages);
        // function_call uses id as both call_id and item_id fallback
        assert_eq!(input[0]["call_id"], "simple_call_id");
        assert_eq!(input[0]["id"], "");
        // function_call_output uses the full id as call_id
        assert_eq!(input[1]["call_id"], "simple_call_id");
    }
}
