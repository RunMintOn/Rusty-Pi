# Ticket A — 修正 `build_context` compaction 处理 + 合成消息体系

**Blocked by:** 3503132 (session persistence) — can start immediately.

**Branch:** `ticket/12-session-build-context-compaction`

## What to build

`build_context` 目前不处理 compaction 和 branch_summary 类型的 entry，导致：

1. Compaction 后的 context 消息列表保留了所有原始消息（含 compacted 之前的），而不是按 `default_context_entry_transform` 过滤
2. CompactionEntry 和 BranchSummaryEntry 在 `build_context` 被静默丢弃，没有转换成合成消息
3. 缺少 `build_context_entries()` 公开方法

修复后 `build_context` 的行为对齐原版 `buildSessionContext`（`reference/earendil-works-pi/packages/agent/src/harness/session/session.ts`）。

## Changes

### 1. 给 `AgentMessage` 加三个合成消息变体（`src/ai/types.rs`）

```rust
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BranchSummary(BranchSummaryMessage),       // 新增
    CompactionSummary(CompactionSummaryMessage), // 新增
    CustomContext(CustomContextMessage),        // 新增
}
```

对应 serde `role` tag：
- `"branchSummary"` → `BranchSummary { summary: String, from_id: String, timestamp: i64 }`
- `"compactionSummary"` → `CompactionSummary { summary: String, tokens_before: u64, timestamp: i64 }`
- `"custom"` → `CustomContext { custom_type: String, content: serde_json::Value, display: bool, details: Option<Value>, timestamp: i64 }`

这些是 context-only 消息——不发送给 LLM、不持久化到 session 树。

### 2. 修正 `Session::build_context`（`src/agent/session/session.rs`）

- 新增 `pub async fn build_context_entries(&self) -> Vec<SessionTreeEntry>` 公开方法，调用 `default_context_entry_transform`
- `build_context` 改为：
  1. 调 `get_branch()` 拿到路径
  2. 调 `default_context_entry_transform` 做 compaction 过滤
  3. 遍历变换后的 entries，把 Message entry 转成 `AgentMessage::User/Assistant/ToolResult`，Compaction entry 转成 `AgentMessage::CompactionSummary`，BranchSummary entry 转成 `AgentMessage::BranchSummary`，等等
  4. 状态提取（thinking_level, model, active_tool_names）保持不变

### 3. 清理 standards 问题（顺手）

- `rand_for_id()` → 改名 `time_hash_for_id()`
- 把各个 `append_*` 方法中重复的 `iso_timestamp()` + `create_entry_id()` + `get_leaf_id()` 模式抽取成私有辅助方法
- 从各个 entry 结构体中移除多余的 `#[serde(skip)] entry_type` 字段（accessor 已按 variant 派发，不需要它）

### 4. 修正测试

- `reconstructs_compaction_summaries_in_context`：更新为 `assert_eq!(context.messages.len(), 4)`，验证第一条消息 role 为 `CompactionSummary`
- 新增 `build_context_entries_uses_default_transform` 测试
- 新增 `branch_summary_appears_in_context` 测试
- 验证 `default_context_entry_transform` 函数被 `build_context` 实际调用

## Acceptance criteria

- [ ] `build_context` 在 compaction 后只保留 compaction entry + 从 `firstKeptEntryId` 开始的条目（匹配原版）
- [ ] `build_context` 返回的消息列表第一条（如果有 compaction）role 为 `compactionSummary`
- [ ] `build_context` 返回的消息列表包含 `branchSummary`（如果 `move_to` 时传了 summary）
- [ ] `build_context_entries()` 公开方法可用
- [ ] 原版 session.test.ts 中所有对应测试的行为在 Rust 版通过
- [ ] `cargo test` 全部通过（123+ → ~127）
- [ ] clippy clean（已有 PR 保证，维持）
