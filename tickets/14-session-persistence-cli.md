# Ticket 14 — Session 持久化串联到 CLI/REPL

## 现状

- `JsonlSessionStorage`（`src/agent/session/jsonl.rs`）已完整实现：create、open、append、query、header 读写、版本管理
- `Session` 业务层（`src/agent/session/session.rs`）已实现：消息追加、context build、compaction transform、branching
- 但 `Agent::new()` （`src/agent/engine.rs:52`）固定创建 `Session::in_memory(cwd)`，每次运行都是全新 in-memory session
- `main.rs` / `repl.rs` 完全没有使用 JSONL 存储，退出即丢失所有对话历史
- `--resume`、`--list-sessions` 等 CLI 参数不存在
- `repl.rs` 内每轮 prompt 在 `run_with_abort` 中通过 `session.agent()` 拿到 `Agent` 引用，但 Agent 不暴露 session 存储切换能力

## 目标

让 rusty-pi 能保存和恢复 session：

- 新运行自动在 `~/.pi/agent/sessions/` 下创建 JSONL 文件
- `--resume <path>`（或部分匹配）恢复已有 session
- `--list-sessions` 列出可用 session（按时间倒序）
- REPL 中每轮 agent 交互后自动 flush 到 JSONL 文件

## Blocked by

None（所有基础设施就绪，只需粘合代码）

## 设计要点

### 1. Agent 需要支持切换存储后端

当前 `Agent::new()` 写死 `Session::in_memory()`。需要添加构造方式让外部传入已存在的 `Session`（带 JSONL 存储）。

**方案：** 给 `Agent` 加 `with_session(session: Session) -> Self`，或在 construction 时允许传入 `Option<Session>`。

参考原版：`AgentSessionRuntime`（`agent-session-runtime.ts`）接受 session manager 和 session data 构建。

### 2. CLI 参数

```rust
/// Resume a previous session (path or partial filename match)
#[arg(short = 'r', long = "resume")]
resume: Option<String>,

/// List available sessions and exit
#[arg(long = "list-sessions")]
list_sessions: bool,
```

### 3. Session 目录

- `~/.pi/agent/sessions/`（由 `get_agent_dir().join("sessions")` 决定）
- 文件名格式：`{session_id}.jsonl`
- `--list-sessions` 读取该目录所有 `.jsonl` 文件，解析 header，按时间倒序列出

### 4. REPL 集成

- 启动时创建或加载 `JsonlSessionStorage`
- 包装为 `Session`，传入 `Agent`
- 每轮 agent `run()` 返回后，session 数据已在内存中（`session.append_message` 写入 in-memory storage）
- 需要在 `JsonlSessionStorage` 上提供 `flush()` 或每次 `append_entry` 同步写文件（当前 `JsonlSessionStorage` 是纯内存+文件追加？查看实现）

查看 `jsonl.rs` 当前实现：`append_entry` 是否同时写入文件？如果是，每次 append 自动持久化；如果不是，需要加 flush 机制或每次 append 后写文件。

## 测试策略

- 单元测试：创建 JSONL session → append 消息 → 关闭 → 重新 open → 验证消息恢复
- 集成测试：CLI `--list-sessions` 输出格式
- Mock：文件系统操作用 tempfile 隔离

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `src/agent/engine.rs` | 添加 `Agent::with_session()` 构造方式，允许外部传入 session |
| `src/main.rs` | 添加 `--resume`、`--list-sessions` 参数；session 路径解析逻辑 |
| `src/coding_agent/repl.rs` | REPL 创建/加载 JSONL session，传入 agent |
| `src/coding_agent/prompt_session.rs` | `PromptSession::new()` 支持接受外部 session 存储 |
| `src/agent/session/jsonl.rs` | 确认 append 写入行为；必要时加 flush 方法 |
