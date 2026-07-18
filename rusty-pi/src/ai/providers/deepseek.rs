//! DeepSeek LLM provider — OpenAI-compatible completions API.
//!
//! Mirrors the original `@earendil-works/pi-ai/src/providers/deepseek.ts`.
//! Uses the standard OpenAI `/chat/completions` endpoint at `https://api.deepseek.com`.

use crate::ai::providers::{Model, ProviderApi};
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;

/// DeepSeek API base URL.
const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";

/// Environment variable for the API key.
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";

/// Available DeepSeek models.
pub const DEEPSEEK_MODELS: &[Model] = &[
    Model {
        id: "deepseek-v4-flash",
        api: "openai-completions",
    },
    Model {
        id: "deepseek-v4-pro",
        api: "openai-completions",
    },
];

/// DeepSeek LLM provider.
pub struct DeepSeekProvider {
    /// API key.
    api_key: String,
    /// Optional custom base URL (defaults to `https://api.deepseek.com`).
    base_url: String,
    /// HTTP client.
    client: reqwest::Client,
}

impl DeepSeekProvider {
    /// Create a new DeepSeek provider using the `DEEPSEEK_API_KEY` environment variable.
    ///
    /// Returns `None` if the environment variable is not set.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(DEEPSEEK_API_KEY_ENV).ok()?;
        Some(Self::new(api_key))
    }

    /// Create a new DeepSeek provider with the given API key.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: DEEPSEEK_BASE_URL.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Set a custom base URL.
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[async_trait]
impl ProviderApi for DeepSeekProvider {
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<AssistantMessage> {
        let url = format!("{}/chat/completions", self.base_url);

        // Build the messages array for the API
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .filter_map(|msg| match msg {
                AgentMessage::User(u) => {
                    let content = match &u.content {
                        crate::ai::types::MessageContent::Text(t) => {
                            serde_json::Value::String(t.clone())
                        }
                        crate::ai::types::MessageContent::Blocks(blocks) => {
                            serde_json::to_value(blocks).unwrap_or_default()
                        }
                    };
                    Some(serde_json::json!({
                        "role": "user",
                        "content": content
                    }))
                }
                AgentMessage::Assistant(a) => {
                    // Concatenate text content for the API
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
                    Some(serde_json::json!({
                        "role": "assistant",
                        "content": text
                    }))
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
            "messages": api_messages,
            "stream": false
        });

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let response_body: serde_json::Value = response.json().await?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("Unknown API error");
            anyhow::bail!("DeepSeek API error ({}): {}", status.as_u16(), error_msg);
        }

        // Parse the response
        let choice = &response_body["choices"][0];
        let message = &choice["message"];
        let content = message["content"].as_str().unwrap_or_default();
        let finish_reason = choice["finish_reason"].as_str().unwrap_or("stop");

        let stop_reason = match finish_reason {
            "stop" => StopReason::Stop,
            "length" => StopReason::Length,
            "tool_calls" => StopReason::ToolUse,
            _ => StopReason::Stop,
        };

        // Parse usage
        let usage = response_body["usage"].as_object().map(|u| {
            crate::ai::types::Usage {
                input: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                output: u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cache_read: 0,
                cache_write: 0,
                total_tokens: u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                cost: crate::ai::types::Cost {
                    input: 0.0,
                    output: 0.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                    total: 0.0,
                },
            }
        });

        let mut assistant_content = Vec::new();
        if !content.is_empty() {
            assistant_content.push(AssistantContent::Text {
                text: content.to_string(),
            });
        }

        // Handle tool calls (if the model supports them)
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for tc in tool_calls {
                let id = tc["id"].as_str().unwrap_or("call_unknown");
                let name = tc["function"]["name"].as_str().unwrap_or("unknown");
                let args = tc["function"]["arguments"].as_str().unwrap_or("{}");
                if let Ok(arguments) = serde_json::from_str(args) {
                    assistant_content.push(AssistantContent::ToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments,
                    });
                }
            }
        }

        Ok(AssistantMessage {
            content: assistant_content,
            api: "openai-completions".into(),
            provider: "deepseek".into(),
            model: model.id.to_string(),
            usage,
            stop_reason,
            error_message: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        })
    }
}
