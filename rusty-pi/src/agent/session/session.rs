//! High-level Session — business logic layer over [`SessionStorage`].
//!
//! Mirrors `Session` from
//! `@earendil-works/pi-agent-core/src/harness/session/session.ts`.

use crate::agent::session::memory::InMemorySessionStorage;
use crate::agent::session::storage::*;
use crate::agent::session::types::*;
use crate::ai::types::{AgentMessage, BranchSummaryMessage, CompactionSummaryMessage, CustomContextMessage};
use std::collections::HashMap;

// ──  Context build options  ──────────────────────────────────────────────

/// Transform applied to context entries after the default compaction transform.
pub type ContextEntryTransform =
    Box<dyn Fn(&[SessionTreeEntry]) -> Vec<SessionTreeEntry> + Send + Sync>;

/// Projector that converts a `CustomEntry` into context messages.
/// Returns `None` / empty vec to omit the entry from context.
pub type CustomEntryContextMessageProjector =
    Box<dyn Fn(&CustomEntry, usize, &[SessionTreeEntry]) -> Vec<AgentMessage> + Send + Sync>;

/// Options for building session context.
/// Mirrors `SessionContextBuildOptions` from the original source.
#[derive(Default)]
pub struct SessionContextBuildOptions {
    /// Entry transforms applied after the default compaction transform.
    pub entry_transforms: Vec<ContextEntryTransform>,
    /// Projectors for custom entries, keyed by `custom_type`.
    /// Custom entries without a matching projector are omitted from context.
    pub entry_projectors: HashMap<String, CustomEntryContextMessageProjector>,
}

/// High-level session wrapper that provides business-logic operations
/// over a [`SessionStorage`] backend.
pub struct Session {
    storage: Box<dyn SessionStorage>,
    context_build_options: SessionContextBuildOptions,
}

impl Session {
    /// Create a new session wrapping the given storage backend.
    pub fn new(
        storage: Box<dyn SessionStorage>,
        context_build_options: SessionContextBuildOptions,
    ) -> Self {
        Self {
            storage,
            context_build_options,
        }
    }

    /// Create a new in-memory session (convenience for Agent use).
    pub fn in_memory(cwd: String) -> Self {
        let now = iso_timestamp();
        let metadata = SessionMetadata {
            id: uuid_v7(),
            created_at: now,
            cwd,
            path: String::new(),
            parent_session_path: None,
            metadata: None,
        };
        Self {
            storage: Box::new(InMemorySessionStorage::new(metadata)),
            context_build_options: SessionContextBuildOptions::default(),
        }
    }

    // ──  Internal helpers  ──────────────────────────────────────────────────

    /// Create an [`EntryBase`] with a new ID, current leaf as parent, and current timestamp.
    async fn new_entry_base(&mut self) -> EntryBase {
        EntryBase {
            id: self.storage.create_entry_id().await,
            parent_id: self.storage.get_leaf_id().await,
            timestamp: iso_timestamp(),
        }
    }

    // ──  Metadata  ─────────────────────────────────────────────────────────

    /// Get the session metadata.
    pub async fn get_metadata(&self) -> SessionMetadata {
        self.storage.get_metadata().await
    }

    // ──  Leaf management  ──────────────────────────────────────────────────

    /// Get the current leaf id.
    pub async fn get_leaf_id(&self) -> Option<String> {
        self.storage.get_leaf_id().await
    }

    /// Move the leaf pointer to a specific entry (tree navigation).
    pub async fn move_to(
        &mut self,
        entry_id: Option<&str>,
        summary: Option<MoveSummary>,
    ) -> Result<Option<String>, SessionError> {
        if let Some(id) = entry_id {
            self.storage
                .get_entry(id)
                .await
                .ok_or_else(|| SessionError::not_found(format!("Entry {} not found", id)))?;
        }

        self.storage
            .set_leaf_id(entry_id.map(|s| s.to_string()))
            .await?;

        if let Some(s) = summary {
            let entry_id = entry_id.unwrap_or("root");
            let mut base = self.new_entry_base().await;
            // Override parent_id — the summary's parent is the entry being moved to
            base.parent_id = Some(entry_id.to_string());
            let entry = SessionTreeEntry::BranchSummary(BranchSummaryEntry {
                base,
                entry_type: EntryTypeTag::BranchSummary,
                from_id: entry_id.to_string(),
                summary: s.summary,
                details: s.details,
                from_hook: s.from_hook,
            });
            let id = entry.id().to_string();
            self.storage.append_entry(entry).await?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    // ──  Entry operations  ─────────────────────────────────────────────────

    /// Append a message entry. Returns the entry ID.
    pub async fn append_message(&mut self, message: AgentMessage) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::Message(MessageEntry {
            base,
            entry_type: EntryTypeTag::Message,
            message,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a model change entry.
    pub async fn append_model_change(
        &mut self,
        provider: String,
        model_id: String,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::ModelChange(ModelChangeEntry {
            base,
            entry_type: EntryTypeTag::ModelChange,
            provider,
            model_id,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a thinking level change entry.
    pub async fn append_thinking_level_change(
        &mut self,
        level: String,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::ThinkingLevelChange(ThinkingLevelChangeEntry {
            base,
            entry_type: EntryTypeTag::ThinkingLevelChange,
            thinking_level: level,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append an active tools change entry.
    pub async fn append_active_tools_change(
        &mut self,
        tool_names: Vec<String>,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::ActiveToolsChange(ActiveToolsChangeEntry {
            base,
            entry_type: EntryTypeTag::ActiveToolsChange,
            active_tool_names: tool_names,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a compaction entry.
    pub async fn append_compaction(
        &mut self,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: u64,
        details: Option<serde_json::Value>,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::Compaction(CompactionEntry {
            base,
            entry_type: EntryTypeTag::Compaction,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook: None,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a custom entry.
    pub async fn append_custom_entry(
        &mut self,
        custom_type: String,
        data: Option<serde_json::Value>,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::Custom(CustomEntry {
            base,
            entry_type: EntryTypeTag::Custom,
            custom_type,
            data,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a custom message entry — a message visible in context with a custom type tag.
    ///
    /// Mirrors `appendCustomMessageEntry` from the original.
    pub async fn append_custom_message_entry(
        &mut self,
        custom_type: String,
        content: serde_json::Value,
        display: bool,
        details: Option<serde_json::Value>,
    ) -> Result<String, SessionError> {
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::CustomMessage(CustomMessageEntry {
            base,
            entry_type: EntryTypeTag::CustomMessage,
            custom_type,
            content,
            display,
            details,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a label entry.
    pub async fn append_label(
        &mut self,
        target_id: String,
        label: Option<String>,
    ) -> Result<String, SessionError> {
        if self.storage.get_entry(&target_id).await.is_none() {
            return Err(SessionError::not_found(format!("Entry {} not found", target_id)));
        }

        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::Label(LabelEntry {
            base,
            entry_type: EntryTypeTag::Label,
            target_id,
            label,
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    /// Append a session info entry (session name).
    pub async fn append_session_name(&mut self, name: String) -> Result<String, SessionError> {
        // Replace runs of \r or \n with a single space (like JS /[\r\n]+/g)
        let mut sanitized = String::with_capacity(name.len());
        let mut in_newline_run = false;
        for c in name.chars() {
            if c == '\r' || c == '\n' {
                if !in_newline_run {
                    sanitized.push(' ');
                    in_newline_run = true;
                }
            } else {
                sanitized.push(c);
                in_newline_run = false;
            }
        }
        let sanitized = sanitized.trim().to_string();
        let base = self.new_entry_base().await;
        let id = base.id.clone();
        let entry = SessionTreeEntry::SessionInfo(SessionInfoEntry {
            base,
            entry_type: EntryTypeTag::SessionInfo,
            name: Some(sanitized),
        });
        self.storage.append_entry(entry).await?;
        Ok(id)
    }

    // ──  Query operations  ─────────────────────────────────────────────────

    /// Get a specific entry by ID.
    pub async fn get_entry(&self, id: &str) -> Option<SessionTreeEntry> {
        self.storage.get_entry(id).await
    }

    /// Get all entries in append order.
    pub async fn get_entries(&self) -> Vec<SessionTreeEntry> {
        self.storage.get_entries().await
    }

    /// Get the branch (path from leaf to root).
    pub async fn get_branch(&self, from_id: Option<&str>) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let leaf_id = match from_id {
            Some(id) => Some(id.to_string()),
            None => self.storage.get_leaf_id().await,
        };
        self.storage
            .get_path_to_root(leaf_id.as_deref())
            .await
    }

    /// Get all messages in the current branch (in order).
    pub async fn messages(&self) -> Vec<AgentMessage> {
        self.get_branch(None)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|entry| match entry {
                SessionTreeEntry::Message(m) => Some(m.message),
                _ => None,
            })
            .collect()
    }

    /// Build the full session context (messages + derived state).
    ///
    /// Applies the default compaction transform, user-provided entry transforms,
    /// and converts all entry types to their corresponding [`AgentMessage`]
    /// representations (including custom entry projectors).
    pub async fn build_context(&self) -> SessionContext {
        let entries = match self.get_branch(None).await {
            Ok(e) => e,
            Err(_) => return SessionContext::default(),
        };

        let state = derive_session_context_state(&entries);
        let context_entries = self.context_entries_with_transforms(&entries);
        let projectors = &self.context_build_options.entry_projectors;
        let messages = context_entries
            .iter()
            .enumerate()
            .flat_map(|(i, entry)| session_entry_to_context_messages(entry, i, &context_entries, projectors))
            .collect();

        SessionContext {
            messages,
            thinking_level: state.thinking_level,
            model: state.model,
            active_tool_names: state.active_tool_names,
        }
    }

    /// Build the context entries (after compaction + user transforms) without converting to messages.
    pub async fn build_context_entries(&self) -> Result<Vec<SessionTreeEntry>, SessionError> {
        let entries = self.get_branch(None).await?;
        Ok(self.context_entries_with_transforms(&entries))
    }

    /// Apply the default compaction transform + user-provided entry transforms.
    fn context_entries_with_transforms(&self, entries: &[SessionTreeEntry]) -> Vec<SessionTreeEntry> {
        let mut result = default_context_entry_transform(entries);
        for transform in &self.context_build_options.entry_transforms {
            result = transform(&result);
        }
        result
    }

    /// Get a label attached to an entry.
    pub async fn get_label(&self, id: &str) -> Option<String> {
        self.storage.get_label(id).await
    }

    /// Get the session name (from the latest session_info entry).
    pub async fn get_session_name(&self) -> Option<String> {
        let entries = self.storage.find_entries(EntryTypeTag::SessionInfo).await;
        entries.last().and_then(|e| match e {
            SessionTreeEntry::SessionInfo(s) => s.name.clone(),
            _ => None,
        })
    }

    /// Derive model from messages (last assistant message's model).
    pub async fn derive_model(&self) -> Option<String> {
        let msgs = self.messages().await;
        msgs.iter().rev().find_map(|m| match m {
            crate::ai::types::AgentMessage::Assistant(a) => Some(a.model.clone()),
            _ => None,
        })
    }

    /// Count messages by type. Returns `(total, user, assistant, tool)`.
    pub async fn count_messages(&self) -> (usize, usize, usize, usize) {
        let msgs = self.messages().await;
        let total = msgs.len();
        let user = msgs.iter().filter(|m| matches!(m, crate::ai::types::AgentMessage::User(_))).count();
        let assistant = msgs.iter().filter(|m| matches!(m, crate::ai::types::AgentMessage::Assistant(_))).count();
        let tool = msgs.iter().filter(|m| matches!(m, crate::ai::types::AgentMessage::ToolResult(_))).count();
        (total, user, assistant, tool)
    }

    /// Clear all entries (reset session).
    pub async fn clear(&mut self) {
        self.storage = Box::new(InMemorySessionStorage::new(self.storage.get_metadata().await));
    }
}

/// Options for [`Session::move_to`] with a branch summary.
pub struct MoveSummary {
    pub summary: String,
    pub details: Option<serde_json::Value>,
    pub from_hook: Option<bool>,
}

// ──  Default context entry transform (compaction handling)  ──────────────

/// Derive state (thinking level, model, active tools) from the original branch entries.
fn derive_session_context_state(entries: &[SessionTreeEntry]) -> SessionContextState {
    let mut thinking_level = String::from("off");
    let mut model: Option<ModelState> = None;
    let mut active_tool_names: Option<Vec<String>> = None;

    for entry in entries {
        match entry {
            SessionTreeEntry::ThinkingLevelChange(t) => {
                thinking_level = t.thinking_level.clone();
            }
            SessionTreeEntry::ModelChange(m) => {
                model = Some(ModelState {
                    provider: m.provider.clone(),
                    model_id: m.model_id.clone(),
                });
            }
            // Also track model from assistant messages (latest wins)
            SessionTreeEntry::Message(m) => {
                if let AgentMessage::Assistant(a) = &m.message {
                    model = Some(ModelState {
                        provider: a.provider.clone(),
                        model_id: a.model.clone(),
                    });
                }
            }
            SessionTreeEntry::ActiveToolsChange(a) => {
                active_tool_names = Some(a.active_tool_names.clone());
            }
            _ => {}
        }
    }

    SessionContextState {
        thinking_level,
        model,
        active_tool_names,
    }
}

/// Transient state extracted from session entries before compaction transform.
struct SessionContextState {
    thinking_level: String,
    model: Option<ModelState>,
    active_tool_names: Option<Vec<String>>,
}

/// Convert a single session tree entry to zero or more context messages.
///
/// Mirrors `sessionEntryToContextMessages` from the original
/// `@earendil-works/pi-agent-core/src/harness/session/session.ts`.
pub fn session_entry_to_context_messages(
    entry: &SessionTreeEntry,
    _index: usize,
    _entries: &[SessionTreeEntry],
    projectors: &HashMap<String, CustomEntryContextMessageProjector>,
) -> Vec<AgentMessage> {
    match entry {
        SessionTreeEntry::Message(m) => vec![m.message.clone()],
        SessionTreeEntry::Compaction(c) => {
            vec![AgentMessage::CompactionSummary(CompactionSummaryMessage {
                summary: c.summary.clone(),
                tokens_before: c.tokens_before,
                timestamp: parse_iso_timestamp(&c.base.timestamp),
            })]
        }
        SessionTreeEntry::BranchSummary(b) if !b.summary.is_empty() => {
            vec![AgentMessage::BranchSummary(BranchSummaryMessage {
                summary: b.summary.clone(),
                from_id: b.from_id.clone(),
                timestamp: parse_iso_timestamp(&b.base.timestamp),
            })]
        }
        SessionTreeEntry::CustomMessage(cm) => {
            vec![AgentMessage::CustomContext(CustomContextMessage {
                custom_type: cm.custom_type.clone(),
                content: cm.content.clone(),
                display: cm.display,
                details: cm.details.clone(),
                timestamp: parse_iso_timestamp(&cm.base.timestamp),
            })]
        }
        SessionTreeEntry::Custom(c) => {
            if let Some(projector) = projectors.get(&c.custom_type) {
                projector(c, _index, _entries)
            } else {
                vec![]
            }
        }
        _ => vec![],
    }
}

/// Default compaction-aware entry transform.
/// Keeps only the compaction marker + entries after `firstKeptEntryId`.
pub fn default_context_entry_transform(
    path_entries: &[SessionTreeEntry],
) -> Vec<SessionTreeEntry> {
    // Find the latest compaction entry
    let compaction_idx = path_entries.iter().rposition(|e| {
        matches!(e, SessionTreeEntry::Compaction(_))
    });

    let Some(ci) = compaction_idx else {
        return path_entries.to_vec();
    };

    let compaction = &path_entries[ci];
    let first_kept = match compaction {
        SessionTreeEntry::Compaction(c) => c.first_kept_entry_id.clone(),
        _ => unreachable!(),
    };

    let mut result = vec![compaction.clone()];
    let mut found_first_kept = false;

    for entry in path_entries.iter().take(ci) {
        if entry.id() == first_kept {
            found_first_kept = true;
        }
        if found_first_kept {
            result.push(entry.clone());
        }
    }

    for entry in path_entries.iter().skip(ci + 1) {
        result.push(entry.clone());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::*;

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

    fn make_session() -> Session {
        Session::in_memory("/test".into())
    }

    #[tokio::test]
    async fn appends_messages_and_builds_context_in_order() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session.append_message(assistant_msg("two")).await.unwrap();
        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 2);
    }

    #[tokio::test]
    async fn tracks_model_and_thinking_level_changes() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session.append_model_change("openai".into(), "gpt-4.1".into()).await.unwrap();
        session.append_thinking_level_change("high".into()).await.unwrap();
        let context = session.build_context().await;
        assert_eq!(context.thinking_level, "high");
        assert_eq!(context.model.as_ref().map(|m| m.provider.as_str()), Some("openai"));
    }

    #[tokio::test]
    async fn supports_branching_by_moving_leaf() {
        let mut session = make_session();
        let user1 = session.append_message(user_msg("one")).await.unwrap();
        let assistant1 = session.append_message(assistant_msg("two")).await.unwrap();
        session.append_message(user_msg("three")).await.unwrap();

        // Move back to user1 and branch
        session.move_to(Some(&user1), None).await.unwrap();
        session.append_message(assistant_msg("branched")).await.unwrap();

        let branch = session.get_branch(None).await.unwrap();
        let ids: Vec<&str> = branch.iter().map(|e| e.id()).collect();
        assert!(ids.contains(&user1.as_str()));
        assert!(!ids.contains(&assistant1.as_str()));
        assert_eq!(branch.len(), 2); // user1 + branched assistant
    }

    #[tokio::test]
    async fn supports_moving_leaf_to_root() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session.move_to(None, None).await.unwrap();
        assert_eq!(session.get_leaf_id().await, None);
        assert_eq!(session.build_context().await.messages.len(), 0);
    }

    #[tokio::test]
    async fn reconstructs_compaction_summaries_in_context() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session.append_message(assistant_msg("two")).await.unwrap();
        let user2 = session.append_message(user_msg("three")).await.unwrap();
        session.append_message(assistant_msg("four")).await.unwrap();
        session.append_compaction("summary".into(), user2, 1234, None).await.unwrap();
        session.append_message(user_msg("five")).await.unwrap();

        let context = session.build_context().await;
        // Messages should include: compactionSummary, three, four, five (compacted messages removed)
        assert_eq!(context.messages.len(), 4);
        // First message is the compaction summary
        match &context.messages[0] {
            AgentMessage::CompactionSummary(cs) => {
                assert_eq!(cs.summary, "summary");
                assert_eq!(cs.tokens_before, 1234);
            }
            _ => panic!("expected compaction summary message, got {:?}", context.messages[0]),
        }
    }

    #[tokio::test]
    async fn supports_moving_with_branch_summary() {
        let mut session = make_session();
        let user1 = session.append_message(user_msg("one")).await.unwrap();

        let summary_id = session
            .move_to(
                Some(&user1),
                Some(MoveSummary {
                    summary: "summary text".into(),
                    details: None,
                    from_hook: None,
                }),
            )
            .await
            .unwrap();
        assert!(summary_id.is_some());

        let entry = session.get_entry(&summary_id.unwrap()).await.unwrap();
        match entry {
            SessionTreeEntry::BranchSummary(ref b) => {
                assert_eq!(b.from_id, user1);
                assert_eq!(b.summary, "summary text");
            }
            _ => panic!("expected branch summary entry"),
        }
    }

    #[tokio::test]
    async fn branch_summary_appears_in_context() {
        let mut session = make_session();
        let user1 = session.append_message(user_msg("one")).await.unwrap();
        session
            .move_to(
                Some(&user1),
                Some(MoveSummary {
                    summary: "branch text".into(),
                    details: None,
                    from_hook: None,
                }),
            )
            .await
            .unwrap();
        session.append_message(assistant_msg("two")).await.unwrap();

        let context = session.build_context().await;
        // Messages: user("one"), branchSummary, assistant("two")
        assert_eq!(context.messages.len(), 3);
        match &context.messages[1] {
            AgentMessage::BranchSummary(bs) => {
                assert_eq!(bs.summary, "branch text");
                assert_eq!(bs.from_id, user1);
            }
            _ => panic!("expected branch summary message, got {:?}", context.messages[1]),
        }
    }

    #[tokio::test]
    async fn build_context_entries_applies_compaction_transform() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        let user2 = session.append_message(user_msg("two")).await.unwrap();
        session.append_compaction("sum".into(), user2, 100, None).await.unwrap();

        let context_entries = session.build_context_entries().await.unwrap();
        // Should include compaction entry + entries from firstKeptEntryId onwards
        assert_eq!(context_entries.len(), 2);
        // After transform, first entry is the compaction marker
        assert!(matches!(context_entries[0], SessionTreeEntry::Compaction(_)));
        assert!(matches!(context_entries[1], SessionTreeEntry::Message(_)));
    }

    #[tokio::test]
    async fn custom_message_entry_appears_in_context() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session
            .append_custom_message_entry("custom_type".into(), serde_json::json!("hello"), true, None)
            .await
            .unwrap();

        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 2);
        match &context.messages[1] {
            AgentMessage::CustomContext(cc) => {
                assert_eq!(cc.custom_type, "custom_type");
                assert!(cc.display);
            }
            _ => panic!("expected custom context message, got {:?}", context.messages[1]),
        }
    }

    #[tokio::test]
    async fn custom_entry_omitted_by_default() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session
            .append_custom_entry("chat_message".into(), Some(serde_json::json!("hello")))
            .await
            .unwrap();

        // Custom entries without a projector are omitted from context
        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 1);
    }

    #[tokio::test]
    async fn custom_entry_projector_injects_messages() {
        let projector: CustomEntryContextMessageProjector =
            Box::new(|entry: &CustomEntry, _index: usize, _entries: &[SessionTreeEntry]| {
                let text = entry.data.as_ref().and_then(|d| d.as_str()).unwrap_or("");
                vec![AgentMessage::User(UserMessage {
                    content: MessageContent::Text(format!("projected: {}", text)),
                    timestamp: 1000,
                })]
            });
        let mut projectors: HashMap<String, CustomEntryContextMessageProjector> = HashMap::new();
        projectors.insert("greeting".into(), projector);

        let opts = SessionContextBuildOptions {
            entry_projectors: projectors,
            ..Default::default()
        };

        let storage = Box::new(InMemorySessionStorage::new(SessionMetadata {
            id: "test-session".into(),
            created_at: "".into(),
            cwd: "/test".into(),
            path: String::new(),
            parent_session_path: None,
            metadata: None,
        }));
        let mut session = Session::new(storage, opts);
        session.append_message(user_msg("one")).await.unwrap();
        session
            .append_custom_entry("greeting".into(), Some(serde_json::json!("hello")))
            .await
            .unwrap();

        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 2);
        match &context.messages[1] {
            AgentMessage::User(u) => {
                assert_eq!(u.content, MessageContent::Text("projected: hello".into()));
            }
            _ => panic!("expected user message from projector"),
        }
    }

    #[tokio::test]
    async fn entry_transform_applied_after_compaction() {
        // Create a transform that drops compaction entries
        let drop_compaction: ContextEntryTransform = Box::new(|entries| {
            entries.iter().filter(|e| !matches!(e, SessionTreeEntry::Compaction(_))).cloned().collect()
        });

        let opts = SessionContextBuildOptions {
            entry_transforms: vec![drop_compaction],
            ..Default::default()
        };

        let storage = Box::new(InMemorySessionStorage::new(SessionMetadata {
            id: "test-session".into(),
            created_at: "".into(),
            cwd: "/test".into(),
            path: String::new(),
            parent_session_path: None,
            metadata: None,
        }));
        let mut session = Session::new(storage, opts);
        session.append_message(user_msg("one")).await.unwrap();
        let user2 = session.append_message(user_msg("two")).await.unwrap();
        session.append_compaction("sum".into(), user2, 100, None).await.unwrap();
        session.append_message(user_msg("three")).await.unwrap();

        // After default compaction transform: [compaction, user("two"), user("three")]
        // After drop_compaction: [user("two"), user("three")]
        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 2);
        match &context.messages[0] {
            AgentMessage::User(u) => assert_eq!(u.content, MessageContent::Text("two".into())),
            _ => panic!("expected user message"),
        }
    }

    #[tokio::test]
    async fn supports_labels_and_session_info() {
        let mut session = make_session();
        let user1 = session.append_message(user_msg("one")).await.unwrap();
        session.append_label(user1.clone(), Some("checkpoint".into())).await.unwrap();
        session.append_session_name("my session".into()).await.unwrap();

        assert_eq!(session.get_label(&user1).await, Some("checkpoint".into()));
        assert_eq!(session.get_session_name().await, Some("my session".into()));

        let context = session.build_context().await;
        assert_eq!(context.messages.len(), 1);
    }

    #[tokio::test]
    async fn rejects_labels_for_missing_entries() {
        let mut session = make_session();
        let err = session
            .append_label("missing".into(), Some("checkpoint".into()))
            .await
            .unwrap_err();
        assert!(matches!(err, SessionError::NotFound(_)));
    }

    #[tokio::test]
    async fn normalizes_session_names() {
        let mut session = make_session();
        session.append_session_name(" hello\nworld\r\nagain ".into()).await.unwrap();
        assert_eq!(session.get_session_name().await, Some("hello world again".into()));
    }

    #[tokio::test]
    async fn clear_resets_session() {
        let mut session = make_session();
        session.append_message(user_msg("one")).await.unwrap();
        session.clear().await;
        assert_eq!(session.get_leaf_id().await, None);
        assert_eq!(session.build_context().await.messages.len(), 0);
    }

    #[tokio::test]
    async fn messages_returns_in_order() {
        let mut session = make_session();
        session.append_message(user_msg("first")).await.unwrap();
        session.append_message(assistant_msg("second")).await.unwrap();

        let msgs = session.messages().await;
        assert_eq!(msgs.len(), 2);
        match &msgs[0] {
            AgentMessage::User(u) => assert_eq!(u.content, MessageContent::Text("first".into())),
            _ => panic!("expected user message"),
        }
    }

    #[tokio::test]
    async fn default_context_entry_transform_no_compaction() {
        let entries: Vec<SessionTreeEntry> = vec![
            SessionTreeEntry::Message(MessageEntry {
                base: EntryBase { id: "e1".into(), parent_id: None, timestamp: "".into() },
                entry_type: EntryTypeTag::Message,
                message: user_msg("hello"),
            }),
        ];
        let result = default_context_entry_transform(&entries);
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn default_context_entry_transform_with_compaction() {
        let entries: Vec<SessionTreeEntry> = vec![
            SessionTreeEntry::Message(MessageEntry {
                base: EntryBase { id: "e1".into(), parent_id: None, timestamp: "".into() },
                entry_type: EntryTypeTag::Message,
                message: user_msg("one"),
            }),
            SessionTreeEntry::Message(MessageEntry {
                base: EntryBase { id: "e2".into(), parent_id: Some("e1".into()), timestamp: "".into() },
                entry_type: EntryTypeTag::Message,
                message: assistant_msg("two"),
            }),
            SessionTreeEntry::Message(MessageEntry {
                base: EntryBase { id: "e3".into(), parent_id: Some("e2".into()), timestamp: "".into() },
                entry_type: EntryTypeTag::Message,
                message: user_msg("three"),
            }),
            SessionTreeEntry::Compaction(CompactionEntry {
                base: EntryBase { id: "compaction".into(), parent_id: Some("e3".into()), timestamp: "".into() },
                entry_type: EntryTypeTag::Compaction,
                summary: "summary".into(),
                first_kept_entry_id: "e2".into(),
                tokens_before: 100,
                details: None,
                from_hook: None,
            }),
            SessionTreeEntry::Message(MessageEntry {
                base: EntryBase { id: "e4".into(), parent_id: Some("compaction".into()), timestamp: "".into() },
                entry_type: EntryTypeTag::Message,
                message: user_msg("four"),
            }),
        ];
        let result = default_context_entry_transform(&entries);
        // Should include: compaction, e2, e3, e4
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].id(), "compaction");
        assert_eq!(result[1].id(), "e2");
    }
}
