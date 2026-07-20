//! Session tree entry types.
//!
//! Mirrors the `SessionTreeEntry` discriminated union from
//! `@earendil-works/pi-agent-core/src/harness/types.ts`.

use crate::ai::types::AgentMessage;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Common base fields
// ---------------------------------------------------------------------------

/// Common fields shared by every session tree entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryBase {
    pub id: String,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

/// A conversation message entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    pub message: AgentMessage,
}

/// Thinking level change entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingLevelChangeEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: String,
}

/// Model change entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelChangeEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    pub provider: String,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Active tools change entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveToolsChangeEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "activeToolNames")]
    pub active_tool_names: Vec<String>,
}

/// Compaction entry — marks where a long conversation was summarized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    pub summary: String,
    #[serde(rename = "firstKeptEntryId")]
    pub first_kept_entry_id: String,
    #[serde(rename = "tokensBefore")]
    pub tokens_before: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Branch summary entry — recorded when the user navigates to a different branch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummaryEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "fromId")]
    pub from_id: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_hook: Option<bool>,
}

/// Custom entry — arbitrary data stored in the session tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "customType")]
    pub custom_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Custom message entry — a message visible in context with a custom type tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMessageEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "customType")]
    pub custom_type: String,
    pub content: serde_json::Value, // string | (TextContent | ImageContent)[]
    pub display: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Label entry — attaches a label to a target entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "targetId")]
    pub target_id: String,
    pub label: Option<String>,
}

/// Session info entry — stores metadata like session name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfoEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Leaf entry — records a move of the active leaf pointer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeafEntry {
    #[serde(flatten)]
    pub base: EntryBase,
    #[serde(skip)]
    pub entry_type: EntryTypeTag,
    #[serde(rename = "targetId")]
    pub target_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Tag enum for JSON type discriminator
// ---------------------------------------------------------------------------

/// The `type` discriminator for JSON serialization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum EntryTypeTag {
    #[serde(rename = "message")]
    #[default]
    Message,
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange,
    #[serde(rename = "model_change")]
    ModelChange,
    #[serde(rename = "active_tools_change")]
    ActiveToolsChange,
    #[serde(rename = "compaction")]
    Compaction,
    #[serde(rename = "branch_summary")]
    BranchSummary,
    #[serde(rename = "custom")]
    Custom,
    #[serde(rename = "custom_message")]
    CustomMessage,
    #[serde(rename = "label")]
    Label,
    #[serde(rename = "session_info")]
    SessionInfo,
    #[serde(rename = "leaf")]
    Leaf,
}

// ---------------------------------------------------------------------------
// Discriminated union
// ---------------------------------------------------------------------------

/// A single entry in the session tree.
///
/// Mirrors `SessionTreeEntry` from the original TypeScript.
/// Each variant has a matching `EntryTypeTag` for JSON round-trips.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionTreeEntry {
    #[serde(rename = "message")]
    Message(MessageEntry),
    #[serde(rename = "thinking_level_change")]
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    #[serde(rename = "model_change")]
    ModelChange(ModelChangeEntry),
    #[serde(rename = "active_tools_change")]
    ActiveToolsChange(ActiveToolsChangeEntry),
    #[serde(rename = "compaction")]
    Compaction(CompactionEntry),
    #[serde(rename = "branch_summary")]
    BranchSummary(BranchSummaryEntry),
    #[serde(rename = "custom")]
    Custom(CustomEntry),
    #[serde(rename = "custom_message")]
    CustomMessage(CustomMessageEntry),
    #[serde(rename = "label")]
    Label(LabelEntry),
    #[serde(rename = "session_info")]
    SessionInfo(SessionInfoEntry),
    #[serde(rename = "leaf")]
    Leaf(LeafEntry),
}

impl SessionTreeEntry {
    pub fn id(&self) -> &str {
        self.base().id.as_str()
    }

    pub fn parent_id(&self) -> Option<&str> {
        self.base().parent_id.as_deref()
    }

    pub fn timestamp(&self) -> &str {
        self.base().timestamp.as_str()
    }

    pub fn entry_type(&self) -> EntryTypeTag {
        match self {
            SessionTreeEntry::Message(_) => EntryTypeTag::Message,
            SessionTreeEntry::ThinkingLevelChange(_) => EntryTypeTag::ThinkingLevelChange,
            SessionTreeEntry::ModelChange(_) => EntryTypeTag::ModelChange,
            SessionTreeEntry::ActiveToolsChange(_) => EntryTypeTag::ActiveToolsChange,
            SessionTreeEntry::Compaction(_) => EntryTypeTag::Compaction,
            SessionTreeEntry::BranchSummary(_) => EntryTypeTag::BranchSummary,
            SessionTreeEntry::Custom(_) => EntryTypeTag::Custom,
            SessionTreeEntry::CustomMessage(_) => EntryTypeTag::CustomMessage,
            SessionTreeEntry::Label(_) => EntryTypeTag::Label,
            SessionTreeEntry::SessionInfo(_) => EntryTypeTag::SessionInfo,
            SessionTreeEntry::Leaf(_) => EntryTypeTag::Leaf,
        }
    }

    fn base(&self) -> &EntryBase {
        match self {
            SessionTreeEntry::Message(e) => &e.base,
            SessionTreeEntry::ThinkingLevelChange(e) => &e.base,
            SessionTreeEntry::ModelChange(e) => &e.base,
            SessionTreeEntry::ActiveToolsChange(e) => &e.base,
            SessionTreeEntry::Compaction(e) => &e.base,
            SessionTreeEntry::BranchSummary(e) => &e.base,
            SessionTreeEntry::Custom(e) => &e.base,
            SessionTreeEntry::CustomMessage(e) => &e.base,
            SessionTreeEntry::Label(e) => &e.base,
            SessionTreeEntry::SessionInfo(e) => &e.base,
            SessionTreeEntry::Leaf(e) => &e.base,
        }
    }
}

// ---------------------------------------------------------------------------
// Session context (derived from the entry tree)
// ---------------------------------------------------------------------------

/// The built context: messages + state derived from metadata entries.
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub messages: Vec<AgentMessage>,
    pub thinking_level: String,
    pub model: Option<ModelState>,
    pub active_tool_names: Option<Vec<String>>,
}

/// Provider/model state tracked through the session tree.
#[derive(Debug, Clone)]
pub struct ModelState {
    pub provider: String,
    pub model_id: String,
}

/// Session header (first line in JSONL file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentSession")]
    pub parent_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Session metadata (derived from header).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub cwd: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentSessionPath")]
    pub parent_session_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a UUID v7-like string.
pub fn uuid_v7() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let random: u64 = time_hash_for_id();
    format!("{:016x}{:016x}", ts, random)
}

/// Generate a short entry ID (last 8 hex chars of uuidv7, with collision retry).
pub fn generate_entry_id(existing_ids: &impl ContainsId) -> String {
    for _ in 0..100 {
        let id = uuid_v7().chars().rev().take(8).collect::<String>();
        if !existing_ids.contains_id(&id) {
            return id;
        }
    }
    uuid_v7()
}

/// Trait for checking ID existence (avoids coupling to a specific container).
pub trait ContainsId {
    fn contains_id(&self, id: &str) -> bool;
}

impl ContainsId for std::collections::HashSet<String> {
    fn contains_id(&self, id: &str) -> bool {
        self.contains(id)
    }
}

impl ContainsId for Vec<String> {
    fn contains_id(&self, id: &str) -> bool {
        self.iter().any(|x| x == id)
    }
}

/// Parse an ISO 8601 timestamp string (as produced by [`iso_timestamp`])
/// into milliseconds since Unix epoch.
pub fn parse_iso_timestamp(s: &str) -> i64 {
    // Expected format: "2026-07-18T12:34:56.789Z"
    if s.len() < 20 || !s.ends_with('Z') {
        return 0;
    }

    let year: i64 = s[0..4].parse().unwrap_or(0);
    let month: u32 = s[5..7].parse().unwrap_or(1);
    let day: u32 = s[8..10].parse().unwrap_or(1);
    let hour: u32 = s[11..13].parse().unwrap_or(0);
    let min: u32 = s[14..16].parse().unwrap_or(0);
    let sec: u32 = s[17..19].parse().unwrap_or(0);

    let millis: i64 = if s.len() > 20 && s.as_bytes()[19] == b'.' {
        let end = s.find('Z').unwrap_or(s.len());
        let ms_str = &s[20..end.min(23)];
        let parsed: i64 = ms_str.parse().unwrap_or(0);
        match ms_str.len() {
            1 => parsed * 100,
            2 => parsed * 10,
            _ => parsed,
        }
    } else {
        0
    };

    let days = date_to_days(year, month, day);
    let total_secs = days * 86400 + (hour as i64) * 3600 + (min as i64) * 60 + sec as i64;
    total_secs * 1000 + millis
}

/// Convert (year, month, day) to days since Unix epoch (1970-01-01).
/// Inverse of [`days_to_date`].
fn date_to_days(year: i64, month: u32, day: u32) -> i64 {
    let m = month as i64;
    let y = if m <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Deterministic time-based hash for ID generation (not cryptographically random).
fn time_hash_for_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mix = t.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((mix >> 32) ^ (mix & 0xFFFF_FFFF)) as u64 & 0x7FFF_FFFF
}

/// Return the leaf id that results from appending an entry.
pub fn leaf_id_after_entry(entry: &SessionTreeEntry) -> Option<String> {
    match entry {
        SessionTreeEntry::Leaf(l) => l.target_id.clone(),
        other => Some(other.id().to_string()),
    }
}

/// Create an ISO 8601 timestamp string (like `new Date().toISOString()`).
pub fn iso_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    // Format as ISO 8601
    let naive = chrono_like_from_unix(secs);
    format!("{}.{:03}Z", naive, millis)
}

/// Minimal UTC date-time formatter without external deps.
fn chrono_like_from_unix(secs: u64) -> String {
    // Days since epoch
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let h = time_secs / 3600;
    let m = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    // Date from days since 1970-01-01
    let (year, month, day) = days_to_date(days as i64);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", year, month, day, h, m, s)
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(mut days: i64) -> (i64, u32, u32) {
    days += 719468; // shift epoch from 1970-01-01 to 0000-03-01
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097; // day of era
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamp_format() {
        let ts = iso_timestamp();
        // Should look like "2026-07-18T12:34:56.789Z"
        assert!(ts.len() > 20, "timestamp too short: {}", ts);
        assert!(ts.ends_with('Z'), "timestamp should end with Z: {}", ts);
        assert_eq!(&ts[4..5], "-", "expected hyphen at position 4: {}", ts);
        assert_eq!(&ts[10..11], "T", "expected T at position 10: {}", ts);
    }

    #[test]
    fn uuid_v7_generates_unique_ids() {
        let ids: std::collections::HashSet<String> = (0..100).map(|_| uuid_v7()).collect();
        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn generate_entry_id_avoids_collisions() {
        let mut existing: std::collections::HashSet<String> = (0..50).map(|i| format!("{:08x}", i)).collect();
        let new_id = generate_entry_id(&existing);
        assert!(!existing.contains(&new_id));
        existing.insert(new_id);
        assert_eq!(existing.len(), 51);
    }

    #[test]
    fn session_tree_entry_roundtrip_message() {
        let entry = SessionTreeEntry::Message(MessageEntry {
            base: EntryBase {
                id: "msg-1".into(),
                parent_id: None,
                timestamp: "2026-01-01T00:00:00.000Z".into(),
            },
            entry_type: EntryTypeTag::Message,
            message: AgentMessage::User(crate::ai::types::UserMessage {
                content: crate::ai::types::MessageContent::Text("hello".into()),
                timestamp: 1000,
            }),
        });
        assert_eq!(entry.id(), "msg-1");
        assert_eq!(entry.entry_type(), EntryTypeTag::Message);

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        assert!(json.contains("\"id\":\"msg-1\""));

        let deserialized: SessionTreeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id(), "msg-1");
        assert_eq!(deserialized.entry_type(), EntryTypeTag::Message);
    }

    #[test]
    fn leaf_id_after_entry_message() {
        let entry = SessionTreeEntry::Message(MessageEntry {
            base: EntryBase {
                id: "e1".into(),
                parent_id: None,
                timestamp: "".into(),
            },
            entry_type: EntryTypeTag::Message,
            message: AgentMessage::User(crate::ai::types::UserMessage {
                content: crate::ai::types::MessageContent::Text("".into()),
                timestamp: 0,
            }),
        });
        assert_eq!(leaf_id_after_entry(&entry), Some("e1".to_string()));
    }

    #[test]
    fn leaf_id_after_entry_leaf() {
        let entry = SessionTreeEntry::Leaf(LeafEntry {
            base: EntryBase {
                id: "leaf-1".into(),
                parent_id: Some("e1".into()),
                timestamp: "".into(),
            },
            entry_type: EntryTypeTag::Leaf,
            target_id: Some("e1".into()),
        });
        assert_eq!(leaf_id_after_entry(&entry), Some("e1".to_string()));
    }

    #[test]
    fn leaf_id_after_entry_leaf_null_target() {
        let entry = SessionTreeEntry::Leaf(LeafEntry {
            base: EntryBase {
                id: "leaf-1".into(),
                parent_id: Some("e1".into()),
                timestamp: "".into(),
            },
            entry_type: EntryTypeTag::Leaf,
            target_id: None,
        });
        assert_eq!(leaf_id_after_entry(&entry), None);
    }

    #[test]
    fn parse_iso_timestamp_standard() {
        let ts = parse_iso_timestamp("2026-07-18T12:34:56.789Z");
        assert_eq!(ts, 1784378096789);
    }

    #[test]
    fn parse_iso_timestamp_zero_padded_millis() {
        let ts = parse_iso_timestamp("2026-01-01T00:00:00.001Z");
        assert_eq!(ts, 1767225600001);
    }

    #[test]
    fn parse_iso_timestamp_no_millis() {
        let ts = parse_iso_timestamp("2026-01-01T00:00:00Z");
        assert!(ts % 1000 == 0);
    }

    #[test]
    fn parse_iso_timestamp_roundtrip() {
        let original = iso_timestamp();
        let millis = parse_iso_timestamp(&original);
        assert!(millis > 0, "expected positive millis from {} got {}", original, millis);
    }
}
