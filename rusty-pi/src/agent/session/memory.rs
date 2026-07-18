//! In-memory session storage backend.
//!
//! Mirrors `InMemorySessionStorage` from
//! `@earendil-works/pi-agent-core/src/harness/session/memory-storage.ts`.

use crate::agent::session::storage::*;
use crate::agent::session::types::*;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};

/// In-memory implementation of [`SessionStorage`].
///
/// All data is kept in memory. Useful for testing and as a default backend.
pub struct InMemorySessionStorage {
    metadata: SessionMetadata,
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, SessionTreeEntry>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl InMemorySessionStorage {
    /// Create a new empty in-memory storage.
    pub fn new(metadata: SessionMetadata) -> Self {
        Self {
            metadata,
            entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            leaf_id: None,
        }
    }

    /// Create with pre-populated entries.
    pub fn with_entries(metadata: SessionMetadata, entries: Vec<SessionTreeEntry>) -> Self {
        let mut by_id = HashMap::new();
        let mut labels_by_id = HashMap::new();
        let mut leaf_id: Option<String> = None;

        for entry in &entries {
            by_id.insert(entry.id().to_string(), entry.clone());
            update_label_cache(&mut labels_by_id, entry);
            leaf_id = leaf_id_after_entry(entry);
        }

        // Validate leaf_id
        if let Some(ref lid) = leaf_id
            && !by_id.contains_key(lid)
        {
            panic!("InMemorySessionStorage: leaf id {} not found in entries", lid);
        }

        Self {
            metadata,
            entries,
            by_id,
            labels_by_id,
            leaf_id,
        }
    }

    /// Validate that `leaf_id` (if Some) points to an existing entry.
    fn validate_leaf(&self) -> Result<(), SessionError> {
        if let Some(ref leaf_id) = self.leaf_id
            && !self.by_id.contains_key(leaf_id)
        {
            return Err(SessionError::invalid_session(format!(
                "Entry {} not found",
                leaf_id
            )));
        }
        Ok(())
    }
}

fn update_label_cache(labels_by_id: &mut HashMap<String, String>, entry: &SessionTreeEntry) {
    if let SessionTreeEntry::Label(label_entry) = entry {
        let label = label_entry.label.as_deref().unwrap_or("").trim();
        if label.is_empty() {
            labels_by_id.remove(&label_entry.target_id);
        } else {
            labels_by_id.insert(label_entry.target_id.clone(), label.to_string());
        }
    }
}

#[async_trait]
impl SessionStorage for InMemorySessionStorage {
    async fn get_metadata(&self) -> SessionMetadata {
        self.metadata.clone()
    }

    async fn get_leaf_id(&self) -> Option<String> {
        self.validate_leaf().ok()?;
        self.leaf_id.clone()
    }

    async fn set_leaf_id(&mut self, leaf_id: Option<String>) -> Result<(), SessionError> {
        if let Some(ref id) = leaf_id
            && !self.by_id.contains_key(id)
        {
            return Err(SessionError::not_found(format!("Entry {} not found", id)));
        }

        let now = iso_timestamp();
        let entry = SessionTreeEntry::Leaf(LeafEntry {
            base: EntryBase {
                id: self.create_entry_id().await,
                parent_id: self.leaf_id.clone(),
                timestamp: now,
            },
            entry_type: EntryTypeTag::Leaf,
            target_id: leaf_id,
        });

        self.append_entry(entry).await
    }

    async fn create_entry_id(&mut self) -> String {
        let existing_ids: HashSet<String> = self.by_id.keys().cloned().collect();
        generate_entry_id(&existing_ids)
    }

    async fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError> {
        let id = entry.id().to_string();
        self.entries.push(entry.clone());
        self.by_id.insert(id, entry.clone());
        update_label_cache(&mut self.labels_by_id, &entry);
        self.leaf_id = leaf_id_after_entry(&entry);
        Ok(())
    }

    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry> {
        self.by_id.get(id).cloned()
    }

    async fn find_entries(&self, entry_type: EntryTypeTag) -> Vec<SessionTreeEntry> {
        self.entries
            .iter()
            .filter(|e| e.entry_type() == entry_type)
            .cloned()
            .collect()
    }

    async fn get_label(&self, id: &str) -> Option<String> {
        self.labels_by_id.get(id).cloned()
    }

    async fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let leaf_id = match leaf_id {
            Some(id) => id.to_string(),
            None => return Ok(Vec::new()),
        };

        let mut path: Vec<SessionTreeEntry> = Vec::new();
        let mut current = self
            .by_id
            .get(&leaf_id)
            .cloned()
            .ok_or_else(|| SessionError::not_found(format!("Entry {} not found", leaf_id)))?;

        loop {
            let parent_id = current.parent_id().map(|s| s.to_string());
            path.push(current);
            match parent_id {
                Some(ref pid) => {
                    current = self
                        .by_id
                        .get(pid)
                        .cloned()
                        .ok_or_else(|| SessionError::invalid_session(format!("Entry {} not found", pid)))?;
                }
                None => break,
            }
        }

        path.reverse();
        Ok(path)
    }

    async fn get_entries(&self) -> Vec<SessionTreeEntry> {
        self.entries.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::*;

    fn test_metadata() -> SessionMetadata {
        SessionMetadata {
            id: "session-1".into(),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            cwd: "/tmp".into(),
            path: "/tmp/session.jsonl".into(),
            parent_session_path: None,
            metadata: None,
        }
    }

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage {
            content: MessageContent::Text(text.into()),
            timestamp: 1000,
        })
    }

    fn assistant_msg(text: &str) -> AgentMessage {
        AgentMessage::Assistant(AssistantMessage {
            content: vec![AssistantContent::Text { text: text.into() }],
            api: "mock".into(),
            provider: "mock".into(),
            model: "mock".into(),
            usage: None,
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 2000,
        })
    }

    fn msg_entry(id: &str, parent_id: Option<&str>, msg: AgentMessage) -> SessionTreeEntry {
        SessionTreeEntry::Message(MessageEntry {
            base: EntryBase {
                id: id.into(),
                parent_id: parent_id.map(|s| s.into()),
                timestamp: "2026-01-01T00:00:00.000Z".into(),
            },
            entry_type: EntryTypeTag::Message,
            message: msg,
        })
    }

    #[tokio::test]
    async fn returns_configured_metadata() {
        let storage = InMemorySessionStorage::new(test_metadata());
        assert_eq!(storage.get_metadata().await.id, "session-1");
    }

    #[tokio::test]
    async fn empty_storage_has_null_leaf() {
        let storage = InMemorySessionStorage::new(test_metadata());
        assert_eq!(storage.get_leaf_id().await, None);
    }

    #[tokio::test]
    async fn appends_entries_and_updates_leaf() {
        let mut storage = InMemorySessionStorage::new(test_metadata());
        let entry = msg_entry("e1", None, user_msg("one"));
        storage.append_entry(entry).await.unwrap();
        assert_eq!(storage.get_leaf_id().await, Some("e1".into()));
    }

    #[tokio::test]
    async fn copies_initial_entries_and_persists_leaf_changes() {
        let entry = msg_entry("e1", None, user_msg("one"));
        let mut storage = InMemorySessionStorage::with_entries(test_metadata(), vec![entry]);
        assert_eq!(storage.get_leaf_id().await, Some("e1".into()));

        // set_leaf_id to null
        storage.set_leaf_id(None).await.unwrap();
        assert_eq!(storage.get_leaf_id().await, None);

        // check a leaf entry was appended
        let entries = storage.get_entries().await;
        let last = entries.last().unwrap();
        assert_eq!(last.entry_type(), EntryTypeTag::Leaf);
        if let SessionTreeEntry::Leaf(l) = last {
            assert_eq!(l.target_id, None);
        } else {
            panic!("expected leaf entry");
        }
    }

    #[tokio::test]
    async fn rejects_invalid_leaf_ids() {
        let mut storage = InMemorySessionStorage::new(test_metadata());
        let err = storage.set_leaf_id(Some("missing".into())).await.unwrap_err();
        assert!(matches!(err, SessionError::NotFound(_)));
    }

    #[tokio::test]
    async fn finds_entries_by_type() {
        let entry = msg_entry("e1", None, user_msg("one"));
        let mut storage = InMemorySessionStorage::with_entries(test_metadata(), vec![entry]);
        let msgs = storage.find_entries(EntryTypeTag::Message).await;
        assert_eq!(msgs.len(), 1);
        assert_eq!(storage.find_entries(EntryTypeTag::SessionInfo).await.len(), 0);

        // append a custom entry
        storage
            .append_entry(SessionTreeEntry::SessionInfo(SessionInfoEntry {
                base: EntryBase {
                    id: "si-1".into(),
                    parent_id: Some("e1".into()),
                    timestamp: "".into(),
                },
                entry_type: EntryTypeTag::SessionInfo,
                name: Some("test".into()),
            }))
            .await
            .unwrap();
        assert_eq!(storage.find_entries(EntryTypeTag::SessionInfo).await.len(), 1);
    }

    #[tokio::test]
    async fn maintains_label_lookup() {
        let entry = msg_entry("e1", None, user_msg("one"));
        let mut storage = InMemorySessionStorage::with_entries(test_metadata(), vec![entry]);
        assert_eq!(storage.get_label("e1").await, None);

        // Add a label
        storage
            .append_entry(SessionTreeEntry::Label(LabelEntry {
                base: EntryBase {
                    id: "label-1".into(),
                    parent_id: Some("e1".into()),
                    timestamp: "2026-01-01T00:00:01.000Z".into(),
                },
                entry_type: EntryTypeTag::Label,
                target_id: "e1".into(),
                label: Some("checkpoint".into()),
            }))
            .await
            .unwrap();
        assert_eq!(storage.get_label("e1").await, Some("checkpoint".into()));

        // Remove label by setting it to None (or empty)
        storage
            .append_entry(SessionTreeEntry::Label(LabelEntry {
                base: EntryBase {
                    id: "label-2".into(),
                    parent_id: Some("label-1".into()),
                    timestamp: "2026-01-01T00:00:02.000Z".into(),
                },
                entry_type: EntryTypeTag::Label,
                target_id: "e1".into(),
                label: None,
            }))
            .await
            .unwrap();
        assert_eq!(storage.get_label("e1").await, None);
    }

    #[tokio::test]
    async fn walks_paths_to_root() {
        let root = msg_entry("root", None, user_msg("root"));
        let child = msg_entry("child", Some("root"), assistant_msg("child"));
        let storage = InMemorySessionStorage::with_entries(test_metadata(), vec![root, child]);
        let path = storage.get_path_to_root(Some("child")).await.unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].id(), "root");
        assert_eq!(path[1].id(), "child");

        let empty = storage.get_path_to_root(None).await.unwrap();
        assert!(empty.is_empty());
    }
}
