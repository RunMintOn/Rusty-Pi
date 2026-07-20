//! Core message types and Tool trait, mirroring `@earendil-works/pi-ai/src/types.ts`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// ── Content blocks (tagged union) ───────────────────────────────────────────

/// Union of all content block types used in messages.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

/// Content blocks allowed in user and tool-result messages (text + image only).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum TextOrImageContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// Content blocks allowed in assistant messages (text + thinking + tool calls).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type")]
pub enum AssistantContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "toolCall")]
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
}

// ── Tool call references ────────────────────────────────────────────────────

/// A tool call extracted from an assistant message for execution.
#[derive(Debug, Clone)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

// ── Usage and cost ──────────────────────────────────────────────────────────

/// Token usage and cost information.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    #[serde(rename = "cacheRead")]
    pub cache_read: u64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: u64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: u64,
    pub cost: Cost,
}

/// Cost breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    pub total: f64,
}

// ── Stop reason ─────────────────────────────────────────────────────────────

/// Reason why an assistant message stopped generating.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum StopReason {
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "length")]
    Length,
    #[serde(rename = "toolUse")]
    ToolUse,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "aborted")]
    Aborted,
}

// ── Messages ────────────────────────────────────────────────────────────────

/// Content of a user message — either a plain string or an array of content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text content.
    Text(String),
    /// Structured content blocks (text + image).
    Blocks(Vec<TextOrImageContent>),
}

/// A message from the user.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UserMessage {
    pub content: MessageContent,
    pub timestamp: i64,
}

/// A message from the assistant (LLM).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssistantMessage {
    pub content: Vec<AssistantContent>,
    pub api: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

/// A message carrying the result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolResultMessage {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: Vec<TextOrImageContent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub timestamp: i64,
}

/// A branch summary message — synthetic message produced during context building.
/// Never sent to the LLM; used to represent a branch navigation event.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BranchSummaryMessage {
    pub summary: String,
    #[serde(rename = "fromId")]
    pub from_id: String,
    pub timestamp: i64,
}

/// A compaction summary message — synthetic message produced during context building.
/// Marks where a long conversation was summarized.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompactionSummaryMessage {
    pub summary: String,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: u64,
    pub timestamp: i64,
}

/// A custom context message — synthetic message produced during context building.
/// Carries arbitrary custom content injected into the model context.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomContextMessage {
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: serde_json::Value,
    pub display: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub timestamp: i64,
}

/// Union of all message types as stored/transmitted.
/// Mirrors the `AgentMessage` union in the original `@earendil-works/pi-agent-core`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "role")]
pub enum AgentMessage {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
    #[serde(rename = "branchSummary")]
    BranchSummary(BranchSummaryMessage),
    #[serde(rename = "compactionSummary")]
    CompactionSummary(CompactionSummaryMessage),
    #[serde(rename = "custom")]
    CustomContext(CustomContextMessage),
}

// ── Tool interface (schema-only) ────────────────────────────────────────────

/// A tool definition exposed to the LLM.
/// Mirrors the `Tool` interface in `@earendil-works/pi-ai/src/types.ts`.
///
/// The `parameters()` method returns a JSON Schema object describing the tool's
/// expected arguments. Tools derive this automatically via `schemars`.
pub trait Tool: Send + Sync {
    /// Tool name (used by the LLM to invoke it).
    fn name(&self) -> &str;
    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;
    /// JSON Schema for the tool's parameters.
    fn parameters(&self) -> serde_json::Value;
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_user_message() {
        let msg = AgentMessage::User(UserMessage {
            content: MessageContent::Text("hello".into()),
            timestamp: 1000,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""role":"user""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn serialize_tool_call_content() {
        let content = Content::ToolCall {
            id: "call_1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "toolCall");
        assert_eq!(json["name"], "bash");
    }

    #[test]
    fn roundtrip_user_message_with_content_blocks() {
        let msg = AgentMessage::User(UserMessage {
            content: MessageContent::Blocks(vec![TextOrImageContent::Text { text: "hello".into() }]),
            timestamp: 2000,
        });
        let json = serde_json::to_value(&msg).unwrap();
        let deserialized: AgentMessage = serde_json::from_value(json).unwrap();
        match deserialized {
            AgentMessage::User(u) => match u.content {
                MessageContent::Blocks(blocks) => {
                    assert_eq!(blocks.len(), 1);
                }
                _ => panic!("expected Blocks variant"),
            },
            _ => panic!("expected User variant"),
        }
    }

    #[test]
    fn roundtrip_assistant_message_with_tool_call() {
        let msg = AgentMessage::Assistant(AssistantMessage {
            content: vec![
                AssistantContent::Text {
                    text: "I'll run that".into(),
                },
                AssistantContent::ToolCall {
                    id: "tc_1".into(),
                    name: "bash".into(),
                    arguments: serde_json::json!({"command": "ls -la"}),
                },
            ],
            api: "openai-completions".into(),
            provider: "deepseek".into(),
            model: "deepseek-v4-pro".into(),
            usage: None,
            stop_reason: StopReason::ToolUse,
            error_message: None,
            timestamp: 3000,
        });
        let json = serde_json::to_value(&msg).unwrap();
        let deserialized: AgentMessage = serde_json::from_value(json).unwrap();
        match deserialized {
            AgentMessage::Assistant(a) => {
                assert_eq!(a.content.len(), 2);
                assert_eq!(a.stop_reason, StopReason::ToolUse);
            }
            _ => panic!("expected Assistant variant"),
        }
    }

    #[test]
    fn schemars_generates_schema_for_user_message() {
        let schema = schemars::schema_for!(UserMessage);
        assert!(schema.schema.metadata.is_some());
    }

    #[test]
    fn text_or_image_content_serialization() {
        let text = TextOrImageContent::Text { text: "hello".into() };
        let json = serde_json::to_value(&text).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "hello");
    }
}
