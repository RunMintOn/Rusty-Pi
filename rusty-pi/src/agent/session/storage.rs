//! Session storage trait and error types.
//!
//! Mirrors `SessionStorage<TMetadata>` and related types from
//! `@earendil-works/pi-agent-core/src/harness/types.ts`.

use crate::agent::session::types::*;
use async_trait::async_trait;
use thiserror::Error;

/// Errors that can occur during session operations.
#[derive(Debug, Clone, Error)]
pub enum SessionError {
    #[error("Entry not found: {0}")]
    NotFound(String),
    #[error("Invalid session: {0}")]
    InvalidSession(String),
    #[error("Invalid entry: {0}")]
    InvalidEntry(String),
    #[error("Invalid fork target: {0}")]
    InvalidForkTarget(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

impl SessionError {
    pub fn not_found(msg: impl Into<String>) -> Self {
        SessionError::NotFound(msg.into())
    }

    pub fn invalid_session(msg: impl Into<String>) -> Self {
        SessionError::InvalidSession(msg.into())
    }

    pub fn invalid_entry(msg: impl Into<String>) -> Self {
        SessionError::InvalidEntry(msg.into())
    }
}

/// Abstract session storage backend.
///
/// Mirrors `SessionStorage<TMetadata>` from the original TypeScript.
/// Both in-memory and JSONL-file implementations implement this trait.
#[async_trait]
pub trait SessionStorage: Send + Sync {
    /// Return the session metadata.
    async fn get_metadata(&self) -> SessionMetadata;

    /// Return the current leaf id, or `None` if the tree is empty.
    async fn get_leaf_id(&self) -> Option<String>;

    /// Move the leaf pointer to `leaf_id` by appending a leaf entry.
    async fn set_leaf_id(&mut self, leaf_id: Option<String>) -> Result<(), SessionError>;

    /// Create a unique entry ID.
    async fn create_entry_id(&mut self) -> String;

    /// Append an entry to the session. Updates leaf and label cache.
    async fn append_entry(&mut self, entry: SessionTreeEntry) -> Result<(), SessionError>;

    /// Look up an entry by ID.
    async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry>;

    /// Find all entries of a given type.
    async fn find_entries(&self, entry_type: EntryTypeTag) -> Vec<SessionTreeEntry>;

    /// Look up the label attached to an entry.
    async fn get_label(&self, id: &str) -> Option<String>;

    /// Walk from `leaf_id` to the root, returning entries in order.
    async fn get_path_to_root(&self, leaf_id: Option<&str>) -> Result<Vec<SessionTreeEntry>, SessionError>;

    /// Return all entries in append order.
    async fn get_entries(&self) -> Vec<SessionTreeEntry>;
}
