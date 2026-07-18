# Ticket 23 — 工具执行结果格式化输出

## 现状

- Agent 的 `on_text` 回调直接 `print!("{}", delta)` 输出裸文本
- ToolCall 结果（bash 输出、edit diff、read 内容）没有格式区隔
- 错误信息直接 `eprintln!`，没有统一风格

## 目标

工具执行结果使用 OutputFormatter 格式化：

```
[Tool: bash]
$ cargo test
   Compiling...
    Finished test
test result: ok. 200 passed
─── 0.5s ───

[Tool: read]
src/main.rs
│  fn main() {
│      println!("hello");
│  }
─── 0.1s ───

[Error: edit failed]
╭─ Error ─────────────────────╮
│ oldText not found in file   │
╰─────────────────────────────╯
```

## 设计

### ToolResultFormatter

在 `src/format/` 中新增工具结果格式化：

```rust
impl OutputFormatter {
    /// 工具开始执行 → 标题行
    pub fn tool_start(&self, name: &str, args: &str) -> String {
        let mut buf = Vec::new();
        Badge::new(name, Color::Cyan).print_to(&mut buf).unwrap();
        write!(buf, " {}", args);
        String::from_utf8_lossy(&buf).to_string()
    }

    /// 工具执行完成 → 分隔线 + 耗时
    pub fn tool_end(&self, name: &str, duration_ms: u64) -> String {
        format!("─── {}.{:01}s ───\n", duration_ms / 1000, (duration_ms % 1000) / 100)
    }

    /// 工具错误 → Alert
    pub fn tool_error(&self, tool: &str, error: &str) -> String {
        let mut buf = Vec::new();
        Alert::error(format!("[{}] {}", tool, error))
            .print_to(&mut buf).unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }
}
```

### 接入点

在 `repl.rs` 的 `run_with_abort` 中，Agent 目前只通过 `on_text` 接收流式文本。需要额外注册：

- `on_tool_start` 回调 → 打印工具标题 + 命令
- `on_tool_end` 回调 → 打印分隔线 + 耗时
- Agent 内部已经有 tool call 执行的追踪逻辑，需要确认是否暴露了这些事件

### 必须补充：Agent 的 on_tool_start / on_tool_end 回调

当前 `Agent`（`engine.rs`）**只有 `on_text` 一个 callback**，tool 执行发生在私有方法
`execute_tool()` 内部，外部完全无法感知。

本 ticket 必须先给 `Agent` 加两个 callback：

```rust
pub type ToolStartCallback = Box<dyn Fn(&str, &str) + Send>;  // (tool_name, args)
pub type ToolEndCallback = Box<dyn Fn(&str, u64) + Send>;     // (tool_name, duration_ms)

pub struct Agent {
    on_text: Option<TextCallback>,
    on_tool_start: Option<ToolStartCallback>,  // 新增
    on_tool_end: Option<ToolEndCallback>,      // 新增
}

impl Agent {
    pub fn on_tool_start<F>(&mut self, cb: F) where F: Fn(&str, &str) + Send + 'static { ... }
    pub fn on_tool_end<F>(&mut self, cb: F) where F: Fn(&str, u64) + Send + 'static { ... }
}
```

然后在 `execute_tool()` 前后调用回调。

> 注意：`execute_tool()` 当前在 `run()` 内部被调用，返回值是 `(AgentMessage, bool)`。
> 需要在调用前后记录时间并触发 callback。
> 同时需要确保 callbacks 被 `Arc<>` 包裹以便在异步上下文中安全调用（参考 `on_text` 的模式）。

## 测试

- `tool_start` / `tool_end` / `tool_error` 输出格式断言
- Mock agent 注入，验证回调被调用

## 文件改动

| 文件 | 改动 |
|---|---|
| `src/format/out.rs` | 添加 tool_start/tool_end/tool_error |
| `src/coding_agent/repl.rs` | 注册工具事件回调，使用 OutputFormatter |
| `src/agent/engine.rs` | 可能需要添加 `on_tool_start`/`on_tool_end` 回调 |

## Blocked by

Ticket 19（format module）
