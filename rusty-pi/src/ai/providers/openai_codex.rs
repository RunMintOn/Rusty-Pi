//! OpenAI Codex provider — ChatGPT Plus/Pro API.

use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::StreamEvent;
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;

const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
const CODEX_TOKEN_ENV: &str = "OPENAI_CODEX_TOKEN";

pub const OPENAI_CODEX_MODELS: &[Model] = &[
    Model { id: "gpt-5.6-sol", api: "openai-codex-responses" },
    Model { id: "gpt-5.6-luna", api: "openai-codex-responses" },
    Model { id: "gpt-5.5", api: "openai-codex-responses" },
    Model { id: "gpt-5.4", api: "openai-codex-responses" },
    Model { id: "gpt-5.4-mini", api: "openai-codex-responses" },
];

pub struct OpenAICodexProvider {
    token: String,
}

impl OpenAICodexProvider {
    pub fn from_env() -> Option<Self> {
        let token = std::env::var(CODEX_TOKEN_ENV).ok()?;
        Some(Self::new(token))
    }

    pub fn new(token: String) -> Self {
        Self { token }
    }

    fn build_input(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
        messages.iter().filter_map(|msg| match msg {
            AgentMessage::User(u) => {
                let content = match &u.content {
                    crate::ai::types::MessageContent::Text(t) => t.clone(),
                    _ => "[content]".to_string(),
                };
                Some(serde_json::json!({ "role": "user", "content": content }))
            }
            AgentMessage::Assistant(a) => {
                let text: String = a.content.iter()
                    .filter_map(|c| if let AssistantContent::Text { text } = c { Some(text.as_str()) } else { None })
                    .collect::<Vec<_>>().join("\n");
                if text.is_empty() { return None; }
                Some(serde_json::json!({ "role": "assistant", "content": text }))
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
                    let id = item["id"].as_str().unwrap_or("call_unknown");
                    let name = item["name"].as_str().unwrap_or("unknown");
                    let args_str = item["arguments"].as_str().unwrap_or("{}");
                    if let Ok(arguments) = serde_json::from_str(args_str) {
                        let _ = tx.send(StreamEvent::ToolCall {
                            id: id.to_string(), name: name.to_string(), arguments,
                        }).await;
                    }
                }
                _ => {}
            }
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;

    let msg = AssistantMessage {
        content: vec![],
        api: "openai-codex-responses".into(), provider: "openai-codex".into(),
        model: model_id.to_string(), usage: None,
        stop_reason: StopReason::Stop, error_message: None, timestamp: now,
    };
    let _ = tx.send(StreamEvent::Done { message: msg }).await;
    Ok(())
}
