# 002 — File Mutation Queue

**Status:** Accepted (2025-01)

**Context:** Write 和 Edit 工具都需要对文件进行修改。在 agent 并行执行 job 或 LLM 并发发出多个 tool call 时，对同一文件路径的并发写入会产生竞态条件——后写入者可能覆盖前一个的改动，或两个写入交错导致文件损坏。

**Decision:** 使用全局 `HashMap<String, Arc<tokio::sync::Mutex<()>>>` 实现 per-path 互斥锁队列。

- 每个绝对路径一个 `tokio::sync::Mutex`，惰性创建
- 写入前获取锁，写入后释放
- 不同路径的写入完全并行，不互相阻塞

**Rejected alternatives:**

| 方案 | 为什么放弃 |
|---|---|
| `oneshot::channel` chain | 需要维护等待链，复杂且容易在错误/panic 时死锁 |
| `tokio::sync::Notify` | 通知丢失问题——如果通知发生在 waiter 注册之前，信号永久丢失，导致后续操作无限等待 |
| `tokio::sync::Semaphore` (permit=1) | 功能等价但比 Mutex 重；Mutex 语义更直接（"我锁住这个文件"） |
| 不串行化 | 竞态会导致数据损坏，不可接受 |

**Consequences:**

- Write 和 Edit 共享同一份队列实现，定义在 `tools/write.rs` 中
- 调用方传入闭包，无需手动管理锁的生命周期
- abort signal 在闭包内部通过检查点而非事件监听器处理，确保 mutation queue 在操作完成前不会被释放（mirrors 原版 TypeScript 行为）
