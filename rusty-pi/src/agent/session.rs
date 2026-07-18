//! Session tree — in-memory session data structure.
//!
//! Mirrors the original JSONL session format:
//! each entry has `id`, `parentId`, and `timestamp`, forming a tree.
//! Walking from leaf to root produces the active context.

use crate::ai::types::AgentMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a session entry.
pub type EntryId = String;

/// A session entry in the tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: i64,
    pub kind: EntryKind,
}

/// The kind of a session entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EntryKind {
    /// A conversation message.
    Message(AgentMessage),
    /// A session header (first entry).
    Header {
        version: u32,
        session_id: String,
        cwd: String,
    },
}

/// In-memory session tree.
///
/// Entries form a tree via `id`/`parent_id` links.
/// The `leaf_id` points to the current position in the tree.
/// Walking from `leaf_id` to the root produces the active context.
#[derive(Debug, Clone)]
pub struct Session {
    /// All entries indexed by id.
    entries: HashMap<EntryId, SessionEntry>,
    /// Current leaf position.
    leaf_id: Option<EntryId>,
    /// Session metadata.
    _header: SessionHeader,
}

/// Session metadata.
#[derive(Debug, Clone)]
pub struct SessionHeader {
    pub session_id: String,
    pub cwd: String,
    pub version: u32,
}

impl Session {
    /// Create a new empty session.
    pub fn new(cwd: String) -> Self {
        let session_id = uuid_v7();
        Self {
            entries: HashMap::new(),
            leaf_id: None,
            _header: SessionHeader {
                session_id: session_id.clone(),
                cwd,
                version: 3,
            },
        }
    }

    /// Add a message entry to the session. Returns the entry id.
    pub fn add_message(&mut self, message: AgentMessage) -> EntryId {
        let id = generate_id(&self.entries);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let entry = SessionEntry {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp,
            kind: EntryKind::Message(message),
        };

        self.entries.insert(id.clone(), entry);
        self.leaf_id = Some(id.clone());
        id
    }

    /// Walk from the current leaf to the root, collecting entries in order.
    pub fn walk(&self) -> Vec<&SessionEntry> {
        let mut path = Vec::new();
        let mut current = self.leaf_id.as_ref();
        while let Some(id) = current {
            if let Some(entry) = self.entries.get(id) {
                path.push(entry);
                current = entry.parent_id.as_ref();
            } else {
                break;
            }
        }
        path.reverse();
        path
    }

    /// Walk and collect only the message entries (in order).
    pub fn messages(&self) -> Vec<&AgentMessage> {
        self.walk()
            .into_iter()
            .filter_map(|e| match &e.kind {
                EntryKind::Message(msg) => Some(msg),
                _ => None,
            })
            .collect()
    }

    /// Get the current leaf id.
    pub fn leaf_id(&self) -> Option<&EntryId> {
        self.leaf_id.as_ref()
    }

    /// Clear all entries (reset session).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.leaf_id = None;
    }
}

/// Generate a short random entry ID (8 hex chars, like the original).
fn generate_id(by_id: &HashMap<EntryId, SessionEntry>) -> EntryId {
    use std::time::{SystemTime, UNIX_EPOCH};
    for _ in 0..100 {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let random = rand_for_id();
        let id = format!("{:08x}", (ts as u64).wrapping_add(random) & 0xFFFF_FFFF);
        if !by_id.contains_key(&id) {
            return id;
        }
    }
    uuid_v7().chars().rev().take(8).collect()
}

/// Simple random number for ID generation.
fn rand_for_id() -> u64 {
    // Use a simple hash of current time + process info
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix the u128 value to produce a smaller u64
    let mix = t.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((mix >> 32) ^ (mix & 0xFFFF_FFFF)) as u64 & 0x7FFF_FFFF
}

/// Generate a UUID v7-like string (simplified).
fn uuid_v7() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let random: u64 = rand_for_id();
    format!("{:016x}{:016x}", ts, random)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{AgentMessage, MessageContent, UserMessage};

    #[test]
    fn session_add_and_walk() {
        let mut session = Session::new("/test".into());
        assert!(session.walk().is_empty());

        let id1 = session.add_message(AgentMessage::User(UserMessage {
            content: MessageContent::Text("hello".into()),
            timestamp: 1000,
        }));
        assert_eq!(session.walk().len(), 1);
        assert_eq!(session.leaf_id(), Some(&id1));

        let id2 = session.add_message(AgentMessage::User(UserMessage {
            content: MessageContent::Text("world".into()),
            timestamp: 2000,
        }));
        assert_eq!(session.walk().len(), 2);
        assert_eq!(session.leaf_id(), Some(&id2));
    }

    #[test]
    fn session_messages_in_order() {
        let mut session = Session::new("/test".into());
        session.add_message(AgentMessage::User(UserMessage {
            content: MessageContent::Text("first".into()),
            timestamp: 1000,
        }));
        session.add_message(AgentMessage::User(UserMessage {
            content: MessageContent::Text("second".into()),
            timestamp: 2000,
        }));

        let msgs = session.messages();
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            AgentMessage::User(u) => assert_eq!(u.content, MessageContent::Text("first".into())),
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn session_clear() {
        let mut session = Session::new("/test".into());
        session.add_message(AgentMessage::User(UserMessage {
            content: MessageContent::Text("test".into()),
            timestamp: 1000,
        }));
        assert_eq!(session.walk().len(), 1);
        session.clear();
        assert!(session.walk().is_empty());
    }
}
