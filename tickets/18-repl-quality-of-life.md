# Ticket 18 — REPL Quality of Life

## 现状

- REPL（`src/coding_agent/repl.rs`）使用裸 `std::io::stdin().read_line()`
- 无命令历史（按上箭头没反应）
- 无行内编辑（退格、光标移动由终端驱动，但无 vi/emacs 模式）
- 无多行输入支持
- 无 `/help` 列出可用命令
- 在 `tokio::task::spawn_blocking` 中同步读取行，不够优雅

## 目标

让 REPL 有基本的交互质量：

- 命令历史（跨 session 保存到文件）
- 行内编辑（退格、光标移动、搜索历史）
- `/help` 列出可用命令
- Ctrl+C 在提示符处优雅退出（当前已有）

## Blocked by

None（独立模块，~300 行）

## 设计要点

### 1. 引入 rustyline

[`rustyline`](https://docs.rs/rustyline/) 是 Rust 最成熟的 GNU Readline 替代品，支持：

- 行编辑（Emacs 和 Vi 模式）
- 历史持久化（自动保存到文件）
- 自定义 completer/hinter
- 异步读取支持

```toml
[dependencies]
rustyline = "15"
```

### 2. 集成替换

当前 REPL 核心是 `tokio::task::spawn_blocking` + `read_line`。替换为 rustyline：

```rust
use rustyline::{DefaultEditor, Result as RlResult};

let mut rl = DefaultEditor::new()?;
// 加载历史
if rl.load_history(&history_path).is_err() {
    // 首次运行，无历史文件
}

loop {
    let line = rl.readline("> ")?;
    rl.add_history_entry(&line)?;
    // ... 处理输入
}

// 保存历史
rl.save_history(&history_path)?;
```

### 3. 历史文件路径

`~/.rusty-pi-history` 或 `~/.pi/agent/repl-history.txt`

### 4. `/help` 命令

列出：
- `/exit` / `/quit` — 退出 REPL
- `/help` — 本帮助
- `/session` — 显示当前 session 信息（如果 ticket 14 已实现）

### 5. 多行输入（可选）

检测输入是否以反斜杠结尾或以 `{` 开始但未闭合：

```rust
// 如果输入以 \ 结尾，继续读取
// 如果光标在行首且输入是 {，开启多行模式直到匹配的 }
```

这可以作为增强暂缓，优先完成 1-4。

### 6. 当前 REPL 中的 Ctrl+C 处理

当前 REPL 在 `run_repl` 中通过 `tokio::select!` 捕获 `ctrl_c()` 信号。使用 rustyline 后，Ctrl+C 默认被 rustyline 捕获（清除当前行）。需要确保 rustyline 的 SIGINT 行为与 agent 的 abort 逻辑兼容。

**方案：** 在 agent 执行期间（`run_with_abort`）保持当前的 `tokio::signal::ctrl_c()` 处理。在提示符处使用 rustyline 的标准行编辑。

## 测试策略

- 交互式测试难自动化。重点测：
  - 历史文件加载/保存 roundtrip
  - `/help` 输出格式
  - Ctrl+C 信号不影响提示符状态

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `src/coding_agent/repl.rs` | 替换 `read_line` 为 rustyline；加历史持久化；加 `/help` |
| `Cargo.toml` | 添加 `rustyline` 依赖 |
