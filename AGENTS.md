# Development Rules

## Project Goal

用 Rust 完整重写 [earendil-works/pi](https://github.com/earendil-works/pi)（一个 AI coding agent），最终产物为 `rusty-pi`。

## Design Principle

**全权参考原实现。** 所有架构决策、接口设计、行为细节，一律以原版 TypeScript 代码为准，不自创、不臆想。原版代码位于 `reference/earendil-works-pi/`。

## Testing

- **测试先行。** 核心逻辑在实现之前先写测试。
- **全部本地运行，全部 mock。** 不使用任何在线 API、LLM 端点、或本机已安装的 pi。LLM provider 层使用 mock provider 返回预设响应，文件系统和进程操作根据测试场景酌情 mock。
- **目标是：Rust 版测试覆盖原版 TypeScript 测试的每一个行为点**，而非直接运行原版测试（原版为 vitest + TypeScript，无法在 Rust 中执行）。
- 非 e2e 测试应能通过 `cargo test` 一条命令全部运行通过。

## Code Quality

- 读透原版代码再动手改。对复杂模块，先完整阅读对应原版文件再做移植。
- 类型安全优先。善用 Rust 的类型系统，避免不必要的 `unwrap()` / `expect()`。
- 错误处理使用 `anyhow` / `thiserror`，不 panic。
- 遵循 Rust 社区惯例（clippy、rustfmt）。

## Working Directory

所有产物（代码、测试、文档、配置文件）均放置于本工作区 `pi-rust/` 下，不散落到系统路径或其他目录。

## Project Structure

```
pi-rust/
├── AGENTS.md               ← 本文件
├── reference/
│   └── earendil-works-pi/  ← 原版 pi 参考代码（只读，不修改）
├── rusty-pi/               ← Rust 项目（待创建）
│   └── ...
```

## Agent skills

### Issue tracker

Issues tracked as local markdown files in the repo root. See `docs/agents/issue-tracker.md`.

### Triage labels

Default labels configured (not actively used — solo project). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context repo. See `docs/agents/domain.md`.

## Multi-Agent Collaboration

本仓库可能同时有多个 agent 在工作。你可能会遇到：

- **文件在你两次工作之间被修改** — 别的 agent 正在处理相邻的 ticket。重新读取文件，理解当前状态，然后决定你的改动如何适应。
- **你依赖的代码发生了变化** — 如果别的 agent 重构了你准备改的模块，先读新代码，调整你的方案，而不是回退别人的改动。
- **测试覆盖率在变化** — 每次跑 `cargo test` 时应看到最全的通过状态。如果别的 agent 引入了测试失败，不要不管——停下来看看是不是你的改动暴露了它的问题。

基本原则：**把其他 agent 视为协作完成任务的人类同事**。它们做的改动和你的一样有效。遇到变化，先读、再适应、不抱怨、不重写。

## Commit

- 不要提交除非用户要求。
- 阶段性的成果确认后，由用户决定何时提交。

## Known Pitfalls（踩坑记录）

### 1. 子进程杀死不完整导致测试死锁

**症状**：`cargo test` 卡死，运行几分钟甚至几小时不结束。

**原因**：在 `bash.rs` 中，`kill_process` 只杀 shell 进程（`sh`），不杀其子进程（如 `sleep`）。被杀的 shell 留下孤儿子进程，这些子进程占用 stdout/stderr 管道的写端，导致 `child.wait()` 和 `read_line()` 永远阻塞。

**修复**：
1. 用 `process_group(0)` 让子进程成为独立进程组的组长
2. 用 `libc::killpg(pgid, SIGKILL)` 杀整个进程组
3. 不要用 `Command::new("kill").spawn()`，它会产生僵尸进程

**教训**：杀进程要杀整个进程组，不能只杀父进程。

### 2. Agent 未传递 abort signal 给工具

**症状**：`agent_cancellation_aborts_long_running_tool` 测试失败。

**原因**：`engine.rs` 的 `execute_tool` 方法调用工具时传了 `signal: None`，工具收不到 abort 信号，无法被取消。

**修复**：
1. 创建 `tokio::sync::watch::channel`
2. 启动后台任务监控 `abort_flag`，状态变化时通过 channel 通知
3. 把 `watch::Receiver` 传递给工具

**教训**：abort/cancel 信号必须端到端传递，不能在中间断掉。

### 3. 测试卡住时如何诊断

```bash
# 查看进程树
pstree -p <test_pid>

# 查看线程状态
cat /proc/<test_pid>/task/*/wchan | sort | uniq -c

# 查看僵尸进程
ps -eo pid,ppid,stat,comm | grep Z
```

如果看到大量 `futex_wait_queue`（等锁）或 `do_epoll_wait`（等 I/O），且有僵尸子进程，很可能是上述问题 1 或 2。
