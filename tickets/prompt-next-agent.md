> Historical document
>
> This file records an earlier planning handoff. Current behavior is defined
> by source code, tests, [docs/capabilities.md](../docs/capabilities.md),
> `SPEC.md`, and accepted ADRs. Do not use this file as the current
> implementation specification.

你是一个 rusty-pi 项目（Rust 版 pi coding agent）的开发 agent。

## 项目状态

MVP 核心已完工：agent loop、session 系统、tools（bash/read/write/edit）、skills、prompt templates、DeepSeek + Codex 两个 provider。
200+ 测试全部通过。代码在 `rusty-pi/src/`。

当前决策：**走"裸终端"路线**（用 sparcli + inquire 做格式化输出和交互选择器，不用 ratatui），
确保 100% 可自动化测试（所有 UI 输出通过 print_to 捕获到 buffer 断言）。

## 参考文档（必读）

- `tickets/spec-bare-terminal-architecture.md` — 整体架构设计、分层、测试策略
- `tickets/crate-reference-bare-terminal.md` — 所有引入 crate 的 API 速查 + 测试模式 + Exa 使用说明

## 任务

完成以下 6 个 ticket，按以下顺序逐个推进。每个 ticket 的详细内容见对应文件。

**依赖链：** 19 → 20 → 21/22（可并行）→ 23/24（依赖 19）

| 编号 | 文件 | 内容 |
|---|---|---|
| 19 | `tickets/19-deps-and-format-module.md` | Cargo.toml 加 sparcli + inquire，创建 `src/format/` 模块，OutputFormatter 骨架 |
| 20 | `tickets/20-repl-command-registry.md` | CommandRegistry 斜杠命令系统，已有命令迁移（rustyline 已就位） |
| 21 | `tickets/21-interactive-selectors.md` | /model 和 /context 命令，inquire 选择器 + Picker trait |
| 22 | `tickets/22-session-display-commands.md` | /session /tree /list-sessions 展示命令 |
| 23 | `tickets/23-tool-output-formatting.md` | 工具执行结果（tool start/end/error）用 sparcli 格式化 |
| 24 | `tickets/24-migrate-output-to-formatter.md` | 全面替换现有 println!/eprintln! 为 OutputFormatter |

## 关键实现模式

### 1. 测试模式

所有格式化函数必须通过 `print_to(&mut buf)` 实现可捕获测试，不要直接写 stdout：

```rust
// ✅ 正确：返回 String，可断言
pub fn session_info(&self, info: &SessionInfo) -> String {
    let mut buf = Vec::new();
    KeyValueList::new()
        .entry("Session", &info.id)
        .print_to(&mut buf).unwrap();
    String::from_utf8_lossy(&buf).to_string()
}

// 测试
let out = formatter.session_info(&info);
assert!(out.contains("session-id"));
```

### 2. 可测试性抽象

对于 inquire（交互选择器）和 rustyline（行编辑），使用 trait 包装：

```rust
// Picker trait — inquire 的抽象
pub trait Picker {
    fn select<T: Display>(&self, prompt: &str, options: Vec<T>) -> Result<T>;
}
pub struct MockPicker;  // 测试用
pub struct RealPicker;  // 生产用，真实调用 inquire
```

```rust
// LineReader trait — rustyline 的抽象
pub trait LineReader {
    fn readline(&mut self, prompt: &str) -> Result<String>;
    fn add_history(&mut self, line: &str);
}
```

### 3. 遵从原版

对复杂逻辑先读原版 TS 参考代码（`reference/earendil-works-pi/packages/`）。不重新发明轮子。

### 4. 增量验证

每次改动后 `cargo build && cargo test`，保持全绿。

## 开发规则

1. **测试先行**：每个 ticket 先写测试再实现。所有测试必须 mock，不调用任何在线 API。
2. **类型安全**：用 thiserror/anyhow，不 panic，避免不必要的 unwrap/expect。
3. **不要修改 reference/ 下的任何文件**，不要修改 `src/tui/` 和 `src/orchestrator/`（占位模块）。
4. 不要重写已有模块——在现有代码上增量修改。
5. 不要引入网络依赖或调用外部 API。

## 相关文件（按模块）

- `rusty-pi/src/main.rs` — CLI 入口
- `rusty-pi/src/coding_agent/repl.rs` — REPL 循环（需要重写）
- `rusty-pi/src/coding_agent/prompt_session.rs` — PromptSession
- `rusty-pi/src/agent/engine.rs` — Agent 核心（需要加 on_tool_start/end 回调）
- `rusty-pi/src/ai/providers/mod.rs` — ProviderApi（需要加 list_models 方法）
- `rusty-pi/src/agent/session/session.rs` — Session 业务层
- `rusty-pi/src/agent/session/storage.rs` — SessionStorage trait
- `rusty-pi/src/format/` — 新建模块
- `rusty-pi/Cargo.toml` — 添加依赖
