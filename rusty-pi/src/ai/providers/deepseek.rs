//! DeepSeek LLM provider — OpenAI-compatible completions API.

//!
//! # Authentication
//!
//! The provider reads the API key from the `DEEPSEEK_API_KEY` environment variable.
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

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::{MessageAccumulator, StreamEvent};
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client;

const DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const DEEPSEEK_API_KEY_ENV: &str = "DEEPSEEK_API_KEY";

/// Models supported by the DeepSeek provider.
pub const DEEPSEEK_MODELS: &[Model] = &[
    Model { id: "deepseek-v4-flash", api: "openai-completions" },
    Model { id: "deepseek-v4-pro", api: "openai-completions" },
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
    /// Create a provider from the `DEEPSEEK_API_KEY` environment variable.
    /// Returns `None` if the variable is not set.
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var(DEEPSEEK_API_KEY_ENV).ok()?;
        Some(Self::new(api_key))
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
    /// and `role: "tool"` messages. Assistant messages without text content are
    /// skipped as no-ops.
    fn build_messages(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
        messages.iter().filter_map(|msg| match msg {
            AgentMessage::User(u) => {
                let content = match &u.content {
                    crate::ai::types::MessageContent::Text(t) => serde_json::Value::String(t.clone()),
                    crate::ai::types::MessageContent::Blocks(blocks) => serde_json::to_value(blocks).unwrap_or_default(),
                };
                Some(serde_json::json!({ "role": "user", "content": content }))
            }
            AgentMessage::Assistant(a) => {
                let text: String = a.content.iter()
                    .filter_map(|c| if let AssistantContent::Text { text } = c { Some(text.as_str()) } else { None })
                    .collect::<Vec<_>>().join("\n");
                if text.is_empty() { return None; }
                let mut msg = serde_json::json!({ "role": "assistant", "content": text });
                let tool_calls: Vec<serde_json::Value> = a.content.iter()
                    .filter_map(|c| match c {
                        AssistantContent::ToolCall { id, name, arguments } => Some(serde_json::json!({
                            "id": id, "type": "function", "function": { "name": name, "arguments": serde_json::to_string(arguments).unwrap_or_default() }
                        })),
                        _ => None,
                    }).collect();
                if !tool_calls.is_empty() { msg["tool_calls"] = serde_json::Value::Array(tool_calls); }
                Some(msg)
            }
            AgentMessage::ToolResult(tr) => {
                let content = tr.content.iter()
                    .filter_map(|c| if let crate::ai::types::TextOrImageContent::Text { text } = c { Some(text.clone()) } else { None })
                    .collect::<Vec<_>>().join("\n");
                Some(serde_json::json!({ "role": "tool", "tool_call_id": tr.tool_call_id, "content": content }))
            }
        }).collect()
    }
}

#[async_trait]
impl ProviderApi for DeepSeekProvider {
    async fn stream(
        &self,
        model: &Model,
        messages: &[AgentMessage],
        _tools: &[&dyn Tool],
    ) -> anyhow::Result<StreamReceiver> {
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let url = format!("{}/chat/completions", self.base_url);
        let api_key = self.api_key.clone();
        let api_messages = Self::build_messages(messages);
        let model_id = model.id.to_string();

        tokio::spawn(async move {
            if let Err(e) = do_stream(&url, &api_key, &model_id, &api_messages, tx).await {
                eprintln!("[deepseek] {}", e);
            }
        });

        Ok(rx)
    }
}

async fn do_stream(
    url: &str, api_key: &str, model_id: &str,
    messages: &[serde_json::Value], tx: tokio::sync::mpsc::Sender<StreamEvent>,
) -> anyhow::Result<()> {
    let http_client = Client::new();
    let body = serde_json::json!({ "model": model_id, "messages": messages, "stream": true, "stream_options": { "include_usage": true } });

    let response = http_client.post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body).send().await?;

    let status = response.status();
    if !status.is_success() {
        let error_body: serde_json::Value = response.json().await.unwrap_or_default();
        let error_msg = error_body["error"]["message"].as_str().unwrap_or("Unknown API error");
        anyhow::bail!("DeepSeek API error ({}): {}", status.as_u16(), error_msg);
    }

    let mut acc = MessageAccumulator::new("openai-completions", "deepseek", model_id);
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();
            if line.is_empty() || line.starts_with(':') || line == "data: [DONE]" { continue; }

            if let Some(json_str) = line.strip_prefix("data: ")
                && let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str)
                    && let Some(choices) = data["choices"].as_array() {
                        for choice in choices {
                            let delta = &choice["delta"];
                            let finish_reason = choice["finish_reason"].as_str();

                            if let Some(content) = delta["content"].as_str()
                                && !content.is_empty() {
                                    let _ = tx.send(StreamEvent::TextDelta { delta: content.to_string() }).await;
                                }

                            if let Some(tc) = delta["tool_calls"].as_array() {
                                for call in tc {
                                    let id = call["id"].as_str().unwrap_or("call_unknown");
                                    let name = call["function"]["name"].as_str().unwrap_or("unknown");
                                    let args_str = call["function"]["arguments"].as_str().unwrap_or("{}");
                                    if let Ok(arguments) = serde_json::from_str(args_str) {
                                        let _ = tx.send(StreamEvent::ToolCall {
                                            id: id.to_string(), name: name.to_string(), arguments,
                                        }).await;
                                    }
                                }
                            }

                            if let Some(reason) = finish_reason
                                && !reason.is_empty() && reason != "null" {
                                    let stop = match reason {
                                        "stop" => StopReason::Stop,
                                        "length" => StopReason::Length,
                                        "tool_calls" => StopReason::ToolUse,
                                        _ => StopReason::Stop,
                                    };
                                    acc.push(StreamEvent::Done {
                                        message: AssistantMessage {
                                            content: vec![],
                                            api: "openai-completions".into(), provider: "deepseek".into(),
                                            model: model_id.to_string(),
                                            usage: None, stop_reason: stop, error_message: None,
                                            timestamp: std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64,
                                        },
                                    });
                                }
                        }
                    }
        }
    }

    let msg = acc.build();
    let _ = tx.send(StreamEvent::Done { message: msg }).await;
    Ok(())
}
