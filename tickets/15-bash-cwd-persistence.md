# Ticket 15 — Bash CWD 持久化

## 现状

- `BashTool`（`src/coding_agent/tools/bash.rs`）在构造时接收 `cwd: String`，但**从不更新**
- 每次 bash 命令都在初始目录执行，`cd` 跨命令不生效
- agent 无法在项目目录间导航：用户在第一次 bash 里 `cd src`，第二次 bash 仍然在项目根目录

参考原版：
- 原版 `bash.ts` 使用 `sessionCwd` 跟踪当前目录
- 每次 bash 执行时 prepend `cd {sessionCwd} && ...`
- 每次 bash 结束后检查输出，提取 CWD 变更
- 原版 `session-cwd.ts` 处理 session CWD 与文件系统之间的同步

## 目标

bash 工具在 agent 多轮调用中持久化 CWD：

- 每次 bash 执行前 prepend `cd {current_cwd} && `
- bash 执行后检测 CWD 是否变化
- CWD 在 agent 的所有工具间共享（read/write/edit 也使用同一 CWD）

## Blocked by

None（独立模块改动）

## 设计要点

### 1. 共享 CWD 状态

CWD 需要从 `BashTool` 提升到 agent 级别：

**推荐方案：** 使用 `Arc<RwLock<PathBuf>>` 作为共享状态，在各工具间共享。bash 更新 CWD 后，read/write/edit 自动使用新目录。

```rust
let shared_cwd: Arc<RwLock<PathBuf>> = Arc::new(RwLock::new(initial_cwd));
```

### 2. CWD 检测策略

参考原版 `bash.ts`，每次 bash 命令：

```rust
// Prepend cd to cached CWD
let full_command = format!("cd {} && {}", shell_escape(&self.cwd), command);

// After execution, run pwd to detect new CWD
// Append to stderr so stdout stays clean
let detection_command = format!("({}) && echo __CWD__:$PWD >&2", full_command);
```

从 stderr 提取 `__CWD__:` 行，更新共享 CWD。或者更简单的做法：子进程的 `current_dir()` 已经启动在正确目录，`cd` 内部的相对路径会自动工作，不需要额外检测——只需在 agent 层面跟踪初始 CWD。

但问题在于子进程内的 `cd /some/absolute/path` 无法被父进程感知。所以需要检测。

简化方案：每次 bash 执行完毕，额外执行 `pwd` 并捕获到 stderr 的一个标记行。

### 3. 工具接口调整

`BashTool::new()` 改为接受 `Arc<RwLock<PathBuf>>` 而非 `String`。同样改造 read/write/edit。

## 测试策略

- bash 执行 `cd /tmp && pwd` → 验证 CWD 更新
- 两次 bash：先 `cd /tmp`，再 `pwd` → 验证输出 `/tmp`
- bash `cd nonexistent` → 验证 CWD 不变
- bash `cd` 后 read 工具应使用新 CWD

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `src/coding_agent/tools/bash.rs` | `new()` 接受共享 CWD；execute 内 prepend `cd`；执行后更新 CWD |
| `src/coding_agent/tools/read.rs` | 使用共享 CWD 替代固定 cwd |
| `src/coding_agent/tools/write.rs` | 使用共享 CWD 替代固定 cwd |
| `src/coding_agent/tools/edit.rs` | 使用共享 CWD 替代固定 cwd |
| `src/main.rs` | 创建共享 CWD |
| `src/coding_agent/prompt_session.rs` | 管理共享 CWD |
