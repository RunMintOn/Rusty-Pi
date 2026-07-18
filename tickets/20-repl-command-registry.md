# Ticket 20 — REPL 接入 rustyline + CommandRegistry

## 现状

- REPL（`src/coding_agent/repl.rs`）已使用 rustyline（`DefaultEditor`）提供行编辑和命令历史
- `/exit`、`/quit`、`/help` 已在 `repl.rs` 中硬编码判断
- 但没有任何可扩展的斜杠命令系统（CommandRegistry）
- 无法注册新命令（如将来的 `/model`、`/session`）

## 目标

1. 实现 `CommandRegistry` trait 和注册/分发机制
2. 将现有硬编码的 `/help`、`/exit`、`/quit` 迁移到 CommandRegistry
3. 各命令输出统一使用 `OutputFormatter`（来自 Ticket 19）
4. 保留 Ctrl+C abort 现有逻辑

> rustylin 行编辑、历史持久化已在之前 ticket 完成。本 ticket 核心是 CommandRegistry。

## 设计

参考 `crate-reference-bare-terminal.md` 第 3 节（rustyline）。

### CommandRegistry

```rust
pub trait Command: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn execute(&self, args: &[&str]) -> Result<()>;
}

pub struct CommandRegistry {
    commands: HashMap<String, Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn register(&mut self, cmd: Box<dyn Command>) {
        self.commands.insert(cmd.name().to_string(), cmd);
    }
    pub fn dispatch(&self, input: &str) -> Result<bool> {
        // 返回 true 表示已处理（是斜杠命令）
        if !input.starts_with('/') { return Ok(false); }
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd_name = parts[0].strip_prefix('/').unwrap_or("");
        let args = parts.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());
        // 查找命令并执行...
    }
}
```

### REPL 改造

```rust
pub async fn run_repl(session: &mut PromptSession) -> Result<()> {
    let mut rl = rustyline::DefaultEditor::new()?;
    let history_path = get_history_path();
    let _ = rl.load_history(&history_path);

    println!("rusty-pi REPL (type '/help' for commands)\n");

    loop {
        let line = rl.readline("> ")?;
        rl.add_history_entry(line.as_str())?;

        if registry.dispatch(&line)? { continue; }

        run_with_abort(session, &line).await;
    }

    rl.save_history(&history_path)?;
}
```

### LineReader trait（可测试性）

参考 `crate-reference-bare-terminal.md` 第 3 节的 `MockLineReader`：

```rust
pub trait LineReader {
    fn readline(&mut self, prompt: &str) -> Result<String, ReadlineError>;
    fn add_history(&mut self, line: &str);
    fn save_history(&mut self) -> Result<()>;
}
```

使 REPL 逻辑脱离 rustyline 的具体实现，测试时注入 `MockLineReader`。

## 测试

- MockLineReader 注入 → 验证多轮 prompt 循环
- MockLineReader 输入 `/help` → 验证打印了帮助信息
- MockLineReader 输入 `/exit` → 验证循环退出
- CommandRegistry 注册/分发/未知命令

## 文件改动

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | 添加 `rustyline`（如果还没有） |
| `src/coding_agent/repl.rs` | 重构：LineReader trait + CommandRegistry + rustyline |
| `src/coding_agent/command.rs` | 新建：Command trait + CommandRegistry + /help /exit |

## Blocked by

Ticket 19（format 模块）
