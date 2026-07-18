//! JSONL file session storage backend.
//!
//! Mirrors `JsonlSessionStorage` from
//! `@earendil-works/pi-agent-core/src/harness/session/jsonl-storage.ts`.
//!
//! Format: first line is a JSON session header, each subsequent line is a
//! JSON-encoded `SessionTreeEntry`.

use crate::agent::session::storage::*;
use crate::agent::session::types::*;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use tokio::io::AsyncWriteExt;

/// JSONL file session storage.
#[derive(Debug)]
pub struct JsonlSessionStorage {
    metadata: SessionMetadata,
    file_path: String,
    entries: Vec<SessionTreeEntry>,
    by_id: HashMap<String, SessionTreeEntry>,
    labels_by_id: HashMap<String, String>,
    leaf_id: Option<String>,
}

impl JsonlSessionStorage {
    /// Create a new JSONL session file.
    pub async fn create(
        file_path: String,
        options: JsonlSessionCreateOptions,
    ) -> Result<Self, SessionError> {
        let now = iso_timestamp();
        let header = SessionHeader {
            entry_type: "session".into(),
            version: 3,
            id: options.session_id.clone(),
            timestamp: now.clone(),
            cwd: options.cwd.clone(),
            parent_session: options.parent_session_path.clone(),
            metadata: options.metadata.clone(),
        };

        let header_json = serde_json::to_string(&header)
            .map_err(|e| SessionError::Storage(format!("Failed to serialize header: {}", e)))?;

        // Write header to file
        let mut file = tokio::fs::File::create(&file_path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to create session file: {}", e)))?;
        file.write_all(header_json.as_bytes())
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to write header: {}", e)))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to write newline: {}", e)))?;

        let metadata = SessionMetadata {
            id: options.session_id.clone(),
            created_at: now,
            cwd: options.cwd.clone(),
            path: file_path.clone(),
            parent_session_path: options.parent_session_path.clone(),
            metadata: options.metadata.clone(),
        };

        Ok(Self {
            metadata,
            file_path,
            entries: Vec::new(),
            by_id: HashMap::new(),
            labels_by_id: HashMap::new(),
            leaf_id: None,
        })
    }

    /// Open an existing JSONL session file.
    pub async fn open(file_path: String) -> Result<Self, SessionError> {
        let content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to read session file: {}", e)))?;

        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.is_empty() {
            return Err(SessionError::invalid_session("missing session header"));
        }

        // Parse header (first line)
        let header: SessionHeader = serde_json::from_str(lines[0])
            .map_err(|e| SessionError::invalid_session(format!("first line is not a valid session header: {}", e)))?;

        // Parse entries (remaining lines)
        let mut entries = Vec::new();
        let mut by_id = HashMap::new();
        let mut labels_by_id = HashMap::new();
        let mut leaf_id: Option<String> = None;

        for (i, line) in lines.iter().enumerate().skip(1) {
            let entry: SessionTreeEntry = serde_json::from_str(line)
                .map_err(|e| {
                    SessionError::invalid_entry(format!(
                        "line {}: {}",
                        i + 1,
                        e
                    ))
                })?;
            let id = entry.id().to_string();
            by_id.insert(id, entry.clone());
            update_label_cache(&mut labels_by_id, &entry);
            leaf_id = leaf_id_after_entry(&entry);
            entries.push(entry);
        }

        let metadata = SessionMetadata {
            id: header.id.clone(),
            created_at: header.timestamp.clone(),
            cwd: header.cwd.clone(),
            path: file_path.clone(),
            parent_session_path: header.parent_session,
            metadata: header.metadata,
        };

        Ok(Self {
            metadata,
            file_path,
            entries,
            by_id,
            labels_by_id,
            leaf_id,
        })
    }

    /// Append a JSON line to the file.
    async fn append_json_line(&self, json: &str) -> Result<(), SessionError> {
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&self.file_path)
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to open session file for append: {}", e)))?;
        file.write_all(json.as_bytes())
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to append entry: {}", e)))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| SessionError::Storage(format!("Failed to write newline: {}", e)))?;
        Ok(())
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

/// Options for creating a new JSONL session.
#[derive(Debug, Clone)]
pub struct JsonlSessionCreateOptions {
    pub session_id: String,
    pub cwd: String,
    pub parent_session_path: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

#[async_trait]
impl SessionStorage for JsonlSessionStorage {
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
        // Serialize to JSON and append to file
        let json = serde_json::to_string(&entry)
            .map_err(|e| SessionError::Storage(format!("Failed to serialize entry: {}", e)))?;
        self.append_json_line(&json).await?;

        // Update in-memory state
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

        let mut path = Vec::new();
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
    use tempfile::tempdir;

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
    async fn writes_header_on_create() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(storage.get_leaf_id().await, None);
        assert_eq!(storage.get_entries().await.len(), 0);

        // Append an entry
        storage
            .append_entry(msg_entry("user-1", None, user_msg("one")))
            .await
            .unwrap();

        // Read file and verify
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);
        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["type"], "session");
        assert_eq!(lines[1], serde_json::to_string(&msg_entry("user-1", None, user_msg("one"))).unwrap());
    }

    #[tokio::test]
    async fn throws_for_missing_files() {
        let err = JsonlSessionStorage::open("/tmp/nonexistent-session.jsonl".into())
            .await
            .unwrap_err();
        assert!(matches!(err, SessionError::Storage(_)));
    }

    #[tokio::test]
    async fn throws_for_malformed_session_header() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("bad.jsonl").to_string_lossy().to_string();
        tokio::fs::write(&file_path, "not json\n").await.unwrap();
        let err = JsonlSessionStorage::open(file_path).await.unwrap_err();
        assert!(matches!(err, SessionError::InvalidSession(_)));
    }

    #[tokio::test]
    async fn creates_and_reads_metadata_from_header() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: Some("/tmp/parent.jsonl".into()),
                metadata: None,
            },
        )
        .await
        .unwrap();

        let metadata = storage.get_metadata().await;
        assert_eq!(metadata.id, "session-1");
        assert_eq!(metadata.cwd, dir.path().to_string_lossy());
        assert_eq!(metadata.path, file_path);
        assert_eq!(metadata.parent_session_path, Some("/tmp/parent.jsonl".into()));

        // Append an entry and re-open
        storage
            .append_entry(msg_entry("user-1", None, user_msg("one")))
            .await
            .unwrap();

        let loaded = JsonlSessionStorage::open(file_path.clone()).await.unwrap();
        assert_eq!(loaded.get_metadata().await.id, "session-1");
    }

    #[tokio::test]
    async fn round_trips_custom_header_metadata() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let meta = serde_json::json!({"profile": "reviewer"});
        let storage = JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: Some(meta.clone()),
            },
        )
        .await
        .unwrap();
        assert_eq!(storage.get_metadata().await.metadata, Some(meta.clone()));

        let loaded = JsonlSessionStorage::open(file_path).await.unwrap();
        assert_eq!(loaded.get_metadata().await.metadata, Some(meta));
    }

    #[tokio::test]
    async fn omits_header_metadata_when_not_provided() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        let header: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert!(!header.as_object().unwrap().contains_key("metadata"));
    }

    #[tokio::test]
    async fn loads_existing_entries_and_reconstructs_leaf() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        let root = msg_entry("root", None, user_msg("root"));
        let child = msg_entry("child", Some("root"), assistant_msg("child"));
        storage.append_entry(root).await.unwrap();
        storage.append_entry(child).await.unwrap();

        let loaded = JsonlSessionStorage::open(file_path.clone()).await.unwrap();
        assert_eq!(loaded.get_leaf_id().await, Some("child".into()));
        assert_eq!(
            loaded.get_entries().await.iter().map(|e| e.id().to_string()).collect::<Vec<_>>(),
            vec!["root", "child"]
        );

        // Set leaf via loaded
        let mut loaded_mut = loaded;
        loaded_mut.set_leaf_id(Some("root".into())).await.unwrap();

        let reloaded = JsonlSessionStorage::open(file_path).await.unwrap();
        assert_eq!(reloaded.get_leaf_id().await, Some("root".into()));

        let entries = reloaded.get_entries().await;
        assert_eq!(entries.last().unwrap().entry_type(), EntryTypeTag::Leaf);
        if let SessionTreeEntry::Leaf(l) = entries.last().unwrap() {
            assert_eq!(l.target_id, Some("root".into()));
        } else {
            panic!("expected leaf entry");
        }
    }

    #[tokio::test]
    async fn finds_entries_by_type() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(
            file_path,
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        storage
            .append_entry(msg_entry("e1", None, user_msg("one")))
            .await
            .unwrap();
        assert_eq!(storage.find_entries(EntryTypeTag::Message).await.len(), 1);
        assert_eq!(storage.find_entries(EntryTypeTag::SessionInfo).await.len(), 0);
    }

    #[tokio::test]
    async fn maintains_label_lookup() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("session.jsonl").to_string_lossy().to_string();
        let mut storage = JsonlSessionStorage::create(
            file_path.clone(),
            JsonlSessionCreateOptions {
                session_id: "session-1".into(),
                cwd: dir.path().to_string_lossy().to_string(),
                parent_session_path: None,
                metadata: None,
            },
        )
        .await
        .unwrap();

        storage
            .append_entry(msg_entry("e1", None, user_msg("one")))
            .await
            .unwrap();
        assert_eq!(storage.get_label("e1").await, None);

        // Add label
        storage
            .append_entry(SessionTreeEntry::Label(LabelEntry {
                base: EntryBase {
                    id: "label-1".into(),
                    parent_id: Some("e1".into()),
                    timestamp: "".into(),
                },
                entry_type: EntryTypeTag::Label,
                target_id: "e1".into(),
                label: Some("checkpoint".into()),
            }))
            .await
            .unwrap();
        assert_eq!(storage.get_label("e1").await, Some("checkpoint".into()));

        // Remove label
        storage
            .append_entry(SessionTreeEntry::Label(LabelEntry {
                base: EntryBase {
                    id: "label-2".into(),
                    parent_id: Some("label-1".into()),
                    timestamp: "".into(),
                },
                entry_type: EntryTypeTag::Label,
                target_id: "e1".into(),
                label: None,
            }))
            .await
            .unwrap();
        assert_eq!(storage.get_label("e1").await, None);

        // Re-open and verify
        let loaded = JsonlSessionStorage::open(file_path).await.unwrap();
        assert_eq!(loaded.get_label("e1").await, None);
    }
}
