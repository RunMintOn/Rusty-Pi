//! OpenAI Codex provider — ChatGPT Plus/Pro API.
//!
//! Mirrors the original `@earendil-works/pi-ai/src/api/openai-codex-responses.ts`
//! and `@earendil-works/pi-ai/src/providers/openai-codex.ts`.
//!
//! Uses HTTP SSE to the Codex backend at `https://chatgpt.com/backend-api/responses`.
//! Auth requires an OAuth session token from a ChatGPT Plus/Pro subscription.
//! Provide it via `OPENAI_CODEX_TOKEN` environment variable.

use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{
    AgentMessage, AssistantContent, AssistantMessage, Cost, StopReason, Tool, Usage,
};
use async_trait::async_trait;

/// Codex backend base URL.
const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";

/// Environment variable for the Codex access token.
const CODEX_TOKEN_ENV: &str = "OPENAI_CODEX_TOKEN";

/// Available Codex models.
pub const OPENAI_CODEX_MODELS: &[Model] = &[
    Model { id: "gpt-5.6-sol", api: "openai-codex-responses" },
    Model { id: "gpt-5.6-luna", api: "openai-codex-responses" },
    Model { id: "gpt-5.5", api: "openai-codex-responses" },
    Model { id: "gpt-5.4", api: "openai-codex-responses" },
    Model { id: "gpt-5.4-mini", api: "openai-codex-responses" },
];

/// OpenAI Codex provider.
pub struct OpenAICodexProvider {
    /// OAuth access token.
    token: String,
    /// HTTP client.
    client: reqwest::Client,
}

impl OpenAICodexProvider {
    /// Create from `OPENAI_CODEX_TOKEN` env var.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var(CODEX_TOKEN_ENV).ok()?;
        Some(Self::new(token))
    }

    /// Create with a manually-provided token.
    pub fn new(token: String) -> Self {
        Self {
            token,
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl ProviderApi for OpenAICodexProvider {
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<AssistantMessage> {
        let url = format!("{}/responses", CODEX_BASE_URL);

        // Build the input array from messages
        let input: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|msg| match msg {
                AgentMessage::User(u) => {
                    let content = match &u.content {
                        crate::ai::types::MessageContent::Text(t) => t.clone(),
                        crate::ai::types::MessageContent::Blocks(_) => {
                            "[content blocks]".to_string()
                        }
                    };
                    Some(serde_json::json!({
                        "role": "user",
                        "content": content
                    }))
                }
                AgentMessage::Assistant(a) => {
                    let text: String = a
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            AssistantContent::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        return None;
                    }
                    // Collect tool calls
                    let tool_calls: Vec<serde_json::Value> = a
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            AssistantContent::ToolCall { id, name, arguments } => {
                                Some(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(arguments).unwrap_or_default()
                                    }
                                }))
                            }
                            _ => None,
                        })
                        .collect();

                    let mut item: serde_json::Value = serde_json::json!({
                        "role": "assistant",
                        "content": text
                    });
                    if !tool_calls.is_empty() {
                        item["tool_calls"] = serde_json::Value::Array(tool_calls);
                    }
                    Some(item)
                }
                AgentMessage::ToolResult(tr) => {
                    let content = tr
                        .content
                        .iter()
                        .filter_map(|c| match c {
                            crate::ai::types::TextOrImageContent::Text { text } => {
                                Some(text.clone())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_call_id,
                        "content": content
                    }))
                }
            })
            .collect();

        let body = serde_json::json!({
            "model": model.id,
            "input": input,
            "stream": false,
            "tools": []
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .header("OpenAI-Beta", "responses=v1")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let response_body: serde_json::Value = response.json().await?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown Codex error");
            anyhow::bail!("Codex API error ({}): {}", status.as_u16(), error_msg);
        }

        // Parse response
        let output = &response_body["output"];
        let mut content = Vec::new();
        let mut stop_reason = StopReason::Stop;

        if let Some(items) = output.as_array() {
            for item in items {
                match item["type"].as_str() {
                    Some("message") => {
                        if let Some(parts) = item["content"].as_array() {
                            for part in parts {
                                if let Some("output_text") = part["type"].as_str() {
                                    let text = part["text"].as_str().unwrap_or("");
                                    if !text.is_empty() {
                                        content.push(AssistantContent::Text {
                                            text: text.to_string(),
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(status) = item["status"].as_str() {
                            match status {
                                "incomplete" => stop_reason = StopReason::Length,
                                "failed" => {
                                    stop_reason = StopReason::Error;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some("function_call") => {
                        let id = item["id"].as_str().unwrap_or("call_unknown");
                        let name = item["name"].as_str().unwrap_or("unknown");
                        let args_str = item["arguments"].as_str().unwrap_or("{}");
                        if let Ok(arguments) = serde_json::from_str(args_str) {
                            content.push(AssistantContent::ToolCall {
                                id: id.to_string(),
                                name: name.to_string(),
                                arguments,
                            });
                            stop_reason = StopReason::ToolUse;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Parse usage
        let usage = response_body["usage"].as_object().map(|u| Usage {
            input: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_read: 0,
            cache_write: 0,
            total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cost: Cost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: 0.0,
            },
        });

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Ok(AssistantMessage {
            content,
            api: "openai-codex-responses".into(),
            provider: "openai-codex".into(),
            model: model.id.to_string(),
            usage,
            stop_reason,
            error_message: None,
            timestamp: now,
        })
    }
}
