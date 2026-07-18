# Ticket B — CustomMessageEntry + entryProjectors / entryTransforms

**Blocked by:** Ticket 12 (合成消息体系 + `build_context_entries`)

**Branch:** `ticket/13-session-custom-entries-projectors-transforms`

## What to build

补齐 `Session` 中缺失的三个特性，使 `build_context` 行为完全对齐原版：

1. **`append_custom_message_entry`** — 写入一条自定义消息 entry，在 context 中渲染为 role `"custom"` 的消息
2. **`entryProjectors`** — 构造函数选项，把 `CustomEntry`（无显示内容）投射成模型可见的合成消息
3. **`entryTransforms`** — 构造函数选项，在 `default_context_entry_transform` 之后附加自定义 entry 变换

## Changes

### 1. `append_custom_message_entry`（`src/agent/session/session.rs`）

新增方法：
```rust
pub async fn append_custom_message_entry(
    &mut self,
    custom_type: String,
    content: serde_json::Value,
    display: bool,
    details: Option<serde_json::Value>,
) -> Result<String, SessionError>
```

写入 `SessionTreeEntry::CustomMessage(CustomMessageEntry)`，在 `build_context` 中通过 `session_entry_to_context_messages` 转换成 `AgentMessage::CustomContext`。

### 2. `Session` 构造函数支持 `entryProjectors`（`src/agent/session/session.rs`）

```rust
pub struct SessionContextBuildOptions {
    pub entry_transforms: Vec<ContextEntryTransform>,
    pub entry_projectors: HashMap<String, CustomEntryContextMessageProjector>,
}

pub type ContextEntryTransform = Box<dyn Fn(&[SessionTreeEntry]) -> Vec<SessionTreeEntry> + Send + Sync>;
pub type CustomEntryContextMessageProjector =
    Box<dyn Fn(&CustomEntry, usize, &[SessionTreeEntry]) -> Vec<AgentMessage> + Send + Sync>;
```

`Session` 构造函数新增 `context_build_options: SessionContextBuildOptions` 参数：
```rust
pub fn new(storage: Box<dyn SessionStorage>, context_build_options: SessionContextBuildOptions) -> Self
```

`build_context` 和 `build_context_entries` 使用这些选项（匹配原版 `mergeContextBuildOptions` 逻辑）。

### 3. `build_context_entries` 应用 entryTransforms（`src/agent/session/session.rs`）

`build_context_entries` 流程变为：
1. 调 `get_branch()` 拿到路径
2. 调 `default_context_entry_transform`
3. 依次应用 `entry_transforms`
4. 返回结果

### 4. `session_entry_to_context_messages` 函数（`src/agent/session/session.rs`）

新增纯函数（原版同名，位于 `session.ts`）：
```rust
pub fn session_entry_to_context_messages(
    entry: &SessionTreeEntry,
    index: usize,
    entries: &[SessionTreeEntry],
    projectors: &HashMap<String, CustomEntryContextMessageProjector>,
) -> Vec<AgentMessage>
```

逻辑：
- `Message` → `[entry.message]`
- `Compaction` → `[AgentMessage::CompactionSummary(...)]`
- `BranchSummary` → `[AgentMessage::BranchSummary(...)]`
- `CustomMessage` → `[AgentMessage::CustomContext(...)]`
- `Custom` → 查 `projectors`，有则调用，无则 `[]`
- 其他类型 → `[]`

`build_context` 使用此函数替代内联的 entry → message 转换。

## Acceptance criteria

- [ ] `append_custom_message_entry` 写入 entry 且在 context 中可见（role: "custom"）
- [ ] `CustomEntry` 默认不出现在 context messages 中（无 projector 时）
- [ ] `entryProjectors` 能把 `CustomEntry` 投射为自定义消息
- [ ] `entryTransforms` 能在 compaction 过滤后做附加变换
- [ ] `build_context_entries` 应用了 entryTransforms
- [ ] 原版 session.test.ts 中 custom_message、projector、transform 对应测试通过
- [ ] `cargo test` 全部通过
- [ ] clippy clean
