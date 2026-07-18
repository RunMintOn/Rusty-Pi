# 003 — Session Storage Architecture

**Status:** Accepted (2025-07)

## Context

原版 pi 的 session 系统支持两种存储后端：InMemorySessionStorage（测试用）和 JsonlSessionStorage（文件持久化），上层由高-level Session 封装业务逻辑。

Rust 版最初的 Session 是单一内存结构（`agent/session.rs`），直接嵌入在 Agent 中，仅支持 add_message/walk/messages。缺少：
- 文件持久化（JSONL）
- 分支管理（moveTo/fork）
- 元数据追踪（model change, thinking level, compaction）

## Decision

采用三层架构镜像原版：

```
SessionStorage trait ← InMemorySessionStorage / JsonlSessionStorage
        ↑
   Session（高层封装，业务逻辑）
        ↑
   Agent（只通过 Session API 交互）
```

- `SessionStorage` trait 定义存储操作（CRUD、leaf 管理、path-to-root 遍历）
- `InMemorySessionStorage` / `JsonlSessionStorage` 分别实现 trait
- `Session` 封装业务逻辑（append_message、build_context、move_to），Agent 只与 Session 交互
- `Session::in_memory(cwd)` 提供便捷构造器，底层使用 `InMemorySessionStorage`

## Design Details

### Entry 类型序列化

`SessionTreeEntry` 枚举使用 `#[serde(tag = "type")]` 作为 JSON 标签联合。每个内层 struct 的 `entry_type: EntryTypeTag` 字段必须加 `#[serde(skip)]`，避免与枚举标签重复。

### 时间戳

Entry 时间戳使用 ISO 8601 字符串（如 `"2026-07-18T12:34:56.789Z"`），匹配 JSONL 文件格式。消息内部的 `AgentMessage.timestamp` 保持 i64 毫秒时间戳不变——两者在不同的抽象层级。

### ID 生成

使用简化的 uuidv7（前 6 字节为时间戳毫秒，后 8 字节随机），entry 级别取最后 8 位 hex 字符，碰撞时重试最多 100 次。

## Rejected Alternatives

| 方案 | 为什么放弃 |
|---|---|
| 单一体 Session（同时做存储和业务逻辑） | 测试时需要 mock 文件操作，违反 seam 原则；无法同时使用内存和文件后端 |
| Session 直接引用文件路径，落后时读写 | InMemory 实现需要无副作用使用，文件路径耦合违反测试隔离 |
| 不使用 EntryTypeTag 字段，仅靠 enum 判别 | 需要在匹配时计算类别，不方便做 find_entries 过滤器；原版 TS 也有 type 字段 |

## Consequences

- Agent 通过 `session.append_message().await`（异步 API）操作会话
- 测试注入 `InMemorySessionStorage`，无需文件系统
- JSONL 文件兼容原版格式 v3，可跨版本读取
