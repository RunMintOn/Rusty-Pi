# Ticket 24 — 全面迁移现有输出到 OutputFormatter

## 现状

当前输出风格不统一：

| 位置 | 当前方式 | 例子 |
|---|---|---|
| `repl.rs` 启动信息 | `println!("rusty-pi REPL...")` | 裸文本 |
| `repl.rs` 错误信息 | `eprintln!` | 无颜色无格式 |
| `repl.rs` 中断提示 | `println!("\n[interrupt: aborting...]")` | 裸文本 |
| `main.rs` 构建错误 | `anyhow::bail!` | 标准错误输出 |
| `prompt_session.rs` | 无输出 | — |
| `agent/engine.rs` | 无直接输出 | — |

## 目标

所有用户可见的输出统一走 OutputFormatter：

```
启动时：
  rusty-pi | deepseek-v4-flash | a1b2c3             ← Badge + KV
  Type '/help' for commands

错误：
  ╭─ Error ────────────────────╮
  │ DEEPSEEK_API_KEY not set   │
  ╰────────────────────────────╯

中断：
  ╭─ Interrupt ────────────────╮
  │ Aborted by user            │
  ╰────────────────────────────╯
```

## 改动点

### repl.rs

```rust
// 启动 banner
pub fn print_startup_banner(provider: &str, model: &str, session_id: &str) -> String {
    format!(
        "{} | {} | {}",
        Badge::new("rusty-pi", Color::Cyan).to_string(),
        Badge::new(model, Color::Green).to_string(),
        session_id
    )
}

// 错误信息统一
pub fn print_error(msg: &str) -> String {
    formatter.error(msg)
}

// 中断信息统一
pub fn print_interrupt() -> String {
    Alert::warning("Aborted by user").to_string()
}
```

### main.rs

在 `build_provider` 等错误路径中，输出风格化错误（而非裸 anyhow::bail）：

```rust
// 当前
anyhow::bail!("DEEPSEEK_API_KEY environment variable not set.")

// 改为
eprintln!("{}", OutputFormatter::default().error(
    "DEEPSEEK_API_KEY environment variable not set."
));
```

### prompt_session.rs

加载模板和 skills 时如果出错，通过 formatter 输出警告（而非静默失败）。

## 可测试性

每个格式化函数都返回 String，可以通过 `.print_to()` 或直接 assert 字符串。

## 测试

- 启动 banner 包含 provider、model、session_id
- 错误信息被 Alert 包裹
- 中断信息被 Alert 包裹

### 范围建议：分两步完成

本 ticket 涉及 6+ 个文件，建议分两步以减少单次改动的风险：

**Step A（核心路径）：**
- `src/coding_agent/repl.rs` —— 启动 banner、错误、中断
- `src/main.rs` —— provider 构建错误
- `src/format/out.rs` —— 添加 `banner()`、`interrupt()`、`provider_error()`

**Step B（边缘路径）：**
- `src/coding_agent/prompt_session.rs` —— 模板/skills 加载警告
- `src/coding_agent/tools/*.rs` —— 工具本身的错误输出
- 其他散落的 `eprintln!`

## 文件改动

| 文件 | 改动 | 步 |
|---|---|---|
| `src/coding_agent/repl.rs` | 启动 banner、错误、中断改用 formatter | A |
| `src/main.rs` | provider 错误改用 formatter | A |
| `src/format/out.rs` | 添加 `banner()`、`interrupt()`、`provider_error()` | A |
| `src/coding_agent/prompt_session.rs` | 加载警告改用 formatter | B |
| `src/coding_agent/tools/*.rs` | 工具错误输出 | B |

## Blocked by

Ticket 19（format module）、Ticket 23（tool output formatting）
