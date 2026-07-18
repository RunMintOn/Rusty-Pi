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
//! Requires a ChatGPT Plus or Pro subscription. There are three auth paths:
//!
//! 1. **Manual token**: Set `OPENAI_CODEX_TOKEN` to a raw JWT obtained from
//!    the ChatGPT web app. This is the quickest option for development.
//! 2. **Stored credentials**: If you've logged in via OAuth before, the
//!    credential file is re-used and tokens are refreshed automatically.
//! 3. **OAuth flow**: Device-code login (headless) or browser login (opens a
//!    local HTTP server at port 1455). See `crate::ai::auth::openai_codex`.
//!
//! # Wire format
//!
//! Uses the OpenAI Responses API (`/responses` endpoint) with typed input
//! items: `message`, `function_call`, `function_call_output`. Tool calls
//! from previous turns are passed as `function_call` items in the request
//! input array.

use crate::ai::auth::openai_codex::{CodexCredential, resolve_codex_token};
use crate::ai::providers::{Model, ProviderApi, StreamReceiver};
use crate::ai::stream::StreamEvent;
use crate::ai::types::{AgentMessage, AssistantContent, AssistantMessage, StopReason, Tool};
use async_trait::async_trait;
use futures_util::StreamExt;

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
/// Token resolution (in priority order):
/// 1. `OPENAI_CODEX_TOKEN` env var
/// 2. Saved OAuth credential file (auto-refreshed when expired)
/// 3. Interactive device-code OAuth login
pub struct OpenAICodexProvider {
    token: std::sync::Mutex<String>,
}

impl OpenAICodexProvider {
    /// Shortcut for `self.token.lock().expect("Codex token mutex poisoned")`.
    fn lock_token(&self) -> std::sync::MutexGuard<'_, String> {
        self.token.lock().expect("Codex token mutex poisoned")
    }

    /// Create a provider from the `OPENAI_CODEX_TOKEN` environment variable,
    /// falling back to stored OAuth credentials. Returns `None` if neither
    /// is available (call `from_oauth` to start the interactive flow).
    pub fn from_env() -> Option<Self> {
        // 1. Env var
        if let Ok(token) = std::env::var(CODEX_TOKEN_ENV)
            && !token.is_empty() {
                return Some(Self { token: std::sync::Mutex::new(token) });
        }
        // 2. Stored credentials
        if let Ok(Some(cred)) = CodexCredential::load()
            && !cred.is_expired() {
                return Some(Self { token: std::sync::Mutex::new(cred.access) });
        }
        None
    }

    /// Create a provider performing the full auth resolution chain:
    /// env var → stored credentials → interactive OAuth login.
    pub async fn from_any() -> anyhow::Result<Self> {
        let token = resolve_codex_token(None).await?;
        Ok(Self { token: std::sync::Mutex::new(token) })
    }

    /// Create a new Codex provider with the given JWT access token.
    pub fn new(token: String) -> Self {
        Self { token: std::sync::Mutex::new(token) }
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
        // Refresh token if expired
        let token = if let Ok(Some(cred)) = CodexCredential::load() {
            if cred.is_expired() {
                use crate::ai::auth::openai_codex::refresh_token;
                match refresh_token(&cred.refresh).await {
                    Ok(new_cred) => {
                        let _ = new_cred.save();
                        *self.lock_token() = new_cred.access.clone();
                        new_cred.access
                    }
                    Err(_) => {
                        // Refresh failed; use existing token
                        self.lock_token().clone()
                    }
                }
            } else {
                let mut guard = self.lock_token();
                if guard.as_str() != cred.access {
                    *guard = cred.access.clone();
                }
                guard.clone()
            }
        } else {
            self.lock_token().clone()
        };

        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let url = format!("{}/responses", CODEX_BASE_URL);
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

/// Manually parse SSE events from an HTTP byte stream and dispatch them as StreamEvents.
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
    if !status.is_success() {
        let error_body: serde_json::Value = response.json().await?;
        let error_msg = error_body["error"]["message"].as_str().unwrap_or("Unknown Codex error");
        anyhow::bail!("Codex API error ({}): {}", status.as_u16(), error_msg);
    }

    let mut stream = response.bytes_stream();
    let mut buf = Vec::<u8>::new();
    let mut has_tool_calls = false;

    // Track partial tool call arguments per output_index
    // Key: output_index, Value: (composite_id, name, partial_json_string)
    let mut partial_tool_calls: std::collections::HashMap<i64, (String, String, String)> =
        std::collections::HashMap::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        buf.extend_from_slice(&chunk);

        // Process complete SSE events (separated by \n\n or \r\n\r\n)
        while let Some(sep_start) = find_double_newline(&buf) {
            let raw_event = buf[..sep_start].to_vec();
            let sep_len = if sep_start + 3 < buf.len() && buf[sep_start] == b'\r'
                && buf[sep_start + 1] == b'\n'
                && buf[sep_start + 2] == b'\r'
                && buf[sep_start + 3] == b'\n' {
                4
            } else {
                2
            };
            buf.drain(..sep_start + sep_len); // skip past separator

            if raw_event.is_empty() || raw_event.iter().all(|&b| b == b'\n' || b == b'\r') {
                continue;
            }

            let raw_str = String::from_utf8_lossy(&raw_event);
            let event_type = raw_str.lines()
                .find_map(|line| line.strip_prefix("event:"))
                .map(|s| s.trim().to_string());
            let data_str = raw_str.lines()
                .find_map(|line| line.strip_prefix("data:"))
                .map(|s| s.trim().to_string());

            let event = event_type.as_deref().unwrap_or("");
            let data = match data_str {
                Some(ref d) => d.as_str(),
                None => "",
            };

            match event {
                "response.created" => {
                    // Just ignore; response ID isn't needed for our event system
                }
                "response.output_item.added" => {
                    if data.is_empty() { continue; }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let output_index = val["output_index"].as_i64().unwrap_or(0);
                        if let Some(item) = val.get("item")
                            && item["type"] == "function_call" {
                                let call_id = item["call_id"].as_str().unwrap_or("call_unknown");
                                let item_id = item["id"].as_str().unwrap_or("");
                                let composite_id = if item_id.is_empty() {
                                    call_id.to_string()
                                } else {
                                    format!("{}|{}", call_id, item_id)
                                };
                                let name = item["name"].as_str().unwrap_or("unknown").to_string();
                                let initial_args = item["arguments"].as_str().unwrap_or("{}").to_string();
                                partial_tool_calls.insert(output_index, (composite_id, name, initial_args));
                            }
                    }
                }
                "response.output_text.delta" => {
                    if data.is_empty() { continue; }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data)
                        && let Some(delta) = val["delta"].as_str()
                        && !delta.is_empty() {
                            let _ = tx.send(StreamEvent::TextDelta { delta: delta.to_string() }).await;
                    }
                }
                "response.function_call_arguments.delta" => {
                    if data.is_empty() { continue; }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let output_index = val["output_index"].as_i64().unwrap_or(0);
                        if let Some(delta) = val["delta"].as_str()
                            && let Some(entry) = partial_tool_calls.get_mut(&output_index) {
                                entry.2.push_str(delta);
                            }
                    }
                }
                "response.output_item.done" => {
                    if data.is_empty() { continue; }
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let output_index = val["output_index"].as_i64().unwrap_or(0);

                        // If this was a tool call, finalize and emit
                        if let Some((composite_id, name, args_str)) = partial_tool_calls.remove(&output_index) {
                            has_tool_calls = true;
                            if let Ok(arguments) = serde_json::from_str(&args_str) {
                                let _ = tx.send(StreamEvent::ToolCall {
                                    id: composite_id,
                                    name,
                                    arguments,
                                }).await;
                            }
                        }
                    }
                }
                "response.completed" | "response.incomplete" => {
                    if data.is_empty() { continue; }
                    let stop_reason = if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let status = val["response"]["status"].as_str().unwrap_or("completed");
                        match status {
                            "incomplete" => StopReason::Length,
                            "failed" | "cancelled" => StopReason::Error,
                            _ => {
                                if has_tool_calls { StopReason::ToolUse } else { StopReason::Stop }
                            }
                        }
                    } else {
                        StopReason::Stop
                    };

                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
                    let msg = AssistantMessage {
                        content: vec![],
                        api: "openai-codex-responses".into(),
                        provider: "openai-codex".into(),
                        model: model_id.to_string(),
                        usage: None,
                        stop_reason,
                        error_message: None,
                        timestamp: now,
                    };
                    let _ = tx.send(StreamEvent::Done { message: msg }).await;
                    return Ok(());
                }
                "error" => {
                    let err_msg = if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        format!("Codex error: {}", val["message"].as_str().unwrap_or("unknown"))
                    } else {
                        "Codex SSE error event".to_string()
                    };
                    let _ = tx.send(StreamEvent::Error {
                        reason: StopReason::Error,
                        message: err_msg.clone(),
                    }).await;
                    anyhow::bail!("{}", err_msg);
                }
                "response.failed" => {
                    let err_msg = if let Ok(val) = serde_json::from_str::<serde_json::Value>(data) {
                        let r = &val["response"];
                        if let Some(error) = r["error"].as_object() {
                            format!("{}: {}",
                                error.get("code").and_then(|c| c.as_str()).unwrap_or("unknown"),
                                error.get("message").and_then(|m| m.as_str()).unwrap_or("no message"))
                        } else if let Some(details) = r["incomplete_details"].as_object() {
                            format!("incomplete: {}", details["reason"].as_str().unwrap_or("unknown"))
                        } else {
                            "Unknown error".to_string()
                        }
                    } else {
                        "Codex response failed".to_string()
                    };
                    let _ = tx.send(StreamEvent::Error {
                        reason: StopReason::Error,
                        message: err_msg.clone(),
                    }).await;
                    anyhow::bail!("{}", err_msg);
                }
                _ => {
                    // Ignore unknown events (like response.in_progress, rate_limit, etc.)
                }
            }
        }
    }

    Ok(())
}

/// Find the position of the first double-newline separator (\n\n or \r\n\r\n)
/// in a byte buffer. Returns the index of the start of the separator.
fn find_double_newline(buf: &[u8]) -> Option<usize> {
    for i in 0..buf.len().saturating_sub(1) {
        if buf[i] == b'\n' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        // Handle \r\n\r\n
        if i + 3 < buf.len() && buf[i] == b'\r' && buf[i + 1] == b'\n'
            && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i);
        }
    }
    None
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

    // -------------------------------------------------------------------------
    // find_double_newline
    // -------------------------------------------------------------------------

    #[test]
    fn find_double_newline_lf() {
        // Basic \n\n at various positions
        assert_eq!(find_double_newline(b"aa\n\n"), Some(2));
        assert_eq!(find_double_newline(b"\n\nbb"), Some(0));
        assert_eq!(find_double_newline(b"a\n\nb"), Some(1));
        // No separator
        assert_eq!(find_double_newline(b"abc"), None);
        assert_eq!(find_double_newline(b"a\n"), None);
        assert_eq!(find_double_newline(b"a\n\n\n"), Some(1)); // first \n\n
        // Empty buffer
        assert_eq!(find_double_newline(b""), None);
    }

    #[test]
    fn find_double_newline_crlf() {
        // b"aa\r\n\r\n" = indices: a(0) a(1) \r(2) \n(3) \r(4) \n(5)
        assert_eq!(find_double_newline(b"aa\r\n\r\n"), Some(2));
        // \r\n\r\nbb = \r(0) \n(1) \r(2) \n(3) b(4) b(5)
        assert_eq!(find_double_newline(b"\r\n\r\nbb"), Some(0));
        // a\r\n\r\nb = a(0) \r(1) \n(2) \r(3) \n(4) b(5)
        assert_eq!(find_double_newline(b"a\r\n\r\nb"), Some(1));
        // Lone \r\n followed by \n\n — matches first pattern at \n\n
        assert_eq!(find_double_newline(b"\r\n\n"), Some(1)); // \n\n starts at index 1
        assert_eq!(find_double_newline(b"\r\nfoo"), None);
    }

    #[test]
    fn find_double_newline_lf_preferred_over_crlf() {
        // When both patterns exist, return the first one
        assert_eq!(find_double_newline(b"a\n\nb\r\n\r\n"), Some(1));
    }

    #[test]
    fn find_double_newline_returns_separator_start() {
        // The return value is the index of the first byte of the separator.
        // For \"aa\n\nbb\": the \n\n starts at index 2.
        let buf = b"aa\n\nbb";
        let sep = find_double_newline(buf).unwrap();
        assert_eq!(sep, 2);
        assert_eq!(&buf[..sep], b"aa");
        assert_eq!(&buf[sep..sep + 2], b"\n\n");

        // For CRLF: \"aa\r\n\r\nbb\" — separator starts at index 2.
        let buf = b"aa\r\n\r\nbb";
        let sep = find_double_newline(buf).unwrap();
        assert_eq!(sep, 2);
        assert_eq!(&buf[..sep], b"aa");
        assert_eq!(&buf[sep..sep + 4], b"\r\n\r\n");
    }

    #[test]
    fn find_double_newline_multipart_stream() {
        // Simulate SSE events separated by \n\n, arriving in chunks.
        // Event 1: "event: a\ndata: {\"x\":1}"
        // Event 2: "event: b\ndata: {\"y\":2}"
        let chunk1 = b"event: a\ndata: {\"x\":1}\n\nevent";
        let sep1 = find_double_newline(chunk1).unwrap();
        assert_eq!(&chunk1[..sep1], b"event: a\ndata: {\"x\":1}");

        let remaining = &chunk1[sep1 + 2..]; // skip \n\n
        assert_eq!(remaining, b"event".as_slice());

        // Complete chunk2
        let chunk2 = [remaining, b": b\ndata: {\"y\":2}\n\n"].concat();
        let sep2 = find_double_newline(&chunk2).unwrap();
        assert_eq!(&chunk2[..sep2], b"event: b\ndata: {\"y\":2}");
    }

    // -------------------------------------------------------------------------
    // SSE buffer processing (do_codex_stream internals)
    // -------------------------------------------------------------------------

    /// Simulate the SSE accumulation and extraction loop used by do_codex_stream.
    /// Feeds byte `chunks` in sequence and returns extracted raw event strings.
    fn extract_sse_events(chunks: Vec<&[u8]>) -> Vec<String> {
        let mut buf = Vec::<u8>::new();
        let mut events = Vec::new();

        for chunk in chunks {
            buf.extend_from_slice(chunk);
            while let Some(sep_start) = find_double_newline(&buf) {
                let raw_event = buf[..sep_start].to_vec();
                let sep_len = if sep_start + 3 < buf.len() && buf[sep_start] == b'\r'
                    && buf[sep_start + 1] == b'\n'
                    && buf[sep_start + 2] == b'\r'
                    && buf[sep_start + 3] == b'\n'
                {
                    4
                } else {
                    2
                };
                buf.drain(..sep_start + sep_len);

                if raw_event.is_empty() || raw_event.iter().all(|&b| b == b'\n' || b == b'\r') {
                    continue;
                }
                events.push(String::from_utf8_lossy(&raw_event).to_string());
            }
        }
        events
    }

    #[test]
    fn sse_extract_single_event() {
        let events = extract_sse_events(vec![
            b"event: a\ndata: hello\n\n",
        ]);
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("event: a"));
        assert!(events[0].contains("data: hello"));
    }

    #[test]
    fn sse_extract_multiple_events() {
        let events = extract_sse_events(vec![
            b"event: a\ndata: 1\n\nevent: b\ndata: 2\n\nevent: c\ndata: 3\n\n",
        ]);
        assert_eq!(events.len(), 3);
        assert!(events[0].contains("data: 1"));
        assert!(events[1].contains("data: 2"));
        assert!(events[2].contains("data: 3"));
    }

    #[test]
    fn sse_extract_chunked() {
        // Same events but arriving in two chunks, split in the middle
        let events = extract_sse_events(vec![
            b"event: a\ndata: 1\n\nevent: b\nda",
            b"ta: 2\n\nevent: c\ndata: 3\n\n",
        ]);
        assert_eq!(events.len(), 3);
        assert!(events[0].contains("data: 1"));
        assert!(events[1].contains("data: 2"));
        assert!(events[2].contains("data: 3"));
    }

    #[test]
    fn sse_extract_with_trailing_data_no_separator() {
        // Partial event at the end (no \n\n) should be left in buffer
        let events = extract_sse_events(vec![
            b"event: a\ndata: 1\n\nevent: b\ndata: 2",
        ]);
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("data: 1"));
    }

    #[test]
    fn sse_extract_with_leading_noise() {
        // Empty leading event (just whitespace / blank lines before first event)
        let events = extract_sse_events(vec![
            b"\n\nevent: a\ndata: 1\n\n",
        ]);
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("data: 1"));
    }

    #[test]
    fn sse_extract_crlf() {
        let events = extract_sse_events(vec![
            b"event: a\r\ndata: 1\r\n\r\nevent: b\r\ndata: 2\r\n\r\n",
        ]);
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("data: 1"));
        assert!(events[1].contains("data: 2"));
        // Verify first raw event still looks correct
        assert!(events[0].contains("event: a"));
    }

    #[test]
    fn sse_extract_crlf_chunked() {
        let events = extract_sse_events(vec![
            b"event: a\r\ndata: 1\r\n\r\nevent: b\r\nda",
            b"ta: 2\r\n\r\n",
        ]);
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("data: 1"));
        assert!(events[1].contains("data: 2"));
    }

    #[test]
    fn sse_extract_empty_chunk_preserves_buffer() {
        let events = extract_sse_events(vec![
            b"event: a\ndata: 1",
            b"",  // empty chunk, should not affect buffer
            b"\n\n",
        ]);
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("data: 1"));
    }
}
