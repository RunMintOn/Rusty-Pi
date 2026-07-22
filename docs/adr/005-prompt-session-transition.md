# 004 — PromptSession: 薄 Session 层而非完整 AgentSession

Prompt Templates 和 Skills 的展开需要在 agent loop 之上加一层编排。原版 TS 有完整的 `AgentSession`（含事件系统、compaction、retry、extension hooks），但我们选择在 Rust 版中只实现一个薄 `PromptSession`，只做两件事：**模板展开 + skill 展开**，不做 session 管理、compaction、retry、extension。

选择薄层而非完整 AgentSession 的原因：

- **当前只需要展开**。其他功能（compaction、retry、steering/followUp 队列）有独立 ticket 跟踪，当前阶段没有它们的消费者。
- **原版职责划分是好的，但不必一次全搬**。`prompt_templates.rs` + `skills.rs` + `system_prompt.rs` 都是从下往上建的底层模块，`PromptSession` 只是隔一层的编排者。后续可以把它越写越厚，最终等价于原版 `AgentSession`。
- **避免阻塞其他工作**。等完整 AgentSession 再接入展开，意味着 templates/skills 在其他功能就绪前完全不可用。

没有选择在 REPL 层直接做展开，因为未来会有多个入口（RPC 模式、batch 模式），展开逻辑应该共享而非复制。
