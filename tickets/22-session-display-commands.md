# Ticket 22 — Session 展示命令：/session、/tree、/list-sessions

## 现状

- Session 存储已实现（ticket 14），CLI 已有 `--resume` 和 `--list-sessions`
- 但 REPL 中没有运行时查看 session 状态的方式
- 没有 `/session`、`/tree`、`/list-sessions` 命令

## 目标

```
> /session
Session:    a1b2c3d4
Model:      deepseek-v4-flash
Messages:   5 (user 2, assistant 2, tool 1)
CWD:        /home/user/project

> /tree
session a1b2c3d4
├── user: write a fibonacci function
├── assistant: fn fib...
│   ├── tool: bash → cargo run  ✓
│   └── tool: write → src/lib.rs  ✓
└── assistant: Done!

> /list-sessions
  Session      │  Model              │  Messages  │  Created
 ─────────────────────────────────────────────────────────────
  a1b2c3d4     │  deepseek-v4-flash  │  5          │  2m ago
  e5f6g7h8     │  deepseek-v4-pro    │  12         │  1h ago
  i9j0k1l2     │  deepseek-coder-v2  │  3          │  yesterday
```

## 设计

### SessionCommand

使用 `OutputFormatter::session_info()`（ticket 19 已实现）：

```rust
impl Command for SessionCommand {
    fn execute(&self, _args: &[&str]) -> Result<()> {
        let info = self.session.get_info();
        let output = self.formatter.session_info(&info);
        println!("{}", output);
        Ok(())
    }
}
```

### TreeCommand

打印 session 树形结构。用 sparcli::Tree 或手写缩进。

从 `Session::get_path_to_root()` 获取条目列表，递归构建树：

```rust
impl Command for TreeCommand {
    fn execute(&self, _args: &[&str]) -> Result<()> {
        let entries = self.session.get_path_to_root(None).await?;
        let tree = build_tree(&entries);
        let mut buf = Vec::new();
        tree.print_to(&mut buf)?;
        print!("{}", String::from_utf8_lossy(&buf));
        Ok(())
    }
}
```

### 前提条件：SessionInfo 结构体与提取方法

`Session` 当前没有 `SessionInfo` 结构体，也没有 `get_info()` 方法。
`/session` 需要 `{id, model, msg_count, cwd}`，但 `Session::get_metadata()` 返回
`SessionMetadata { id, created_at, cwd, ... }`——没有 `model`，没有 `msg_count`。

执行本 ticket 前必须先补充：

```rust
pub struct SessionInfo {
    pub id: String,
    pub model: String,
    pub msg_count: usize,
    pub msg_count_user: usize,
    pub msg_count_assistant: usize,
    pub msg_count_tool: usize,
    pub cwd: String,
}

impl Session {
    pub async fn get_info(&self) -> SessionInfo {
        let meta = self.get_metadata().await;
        let msgs = self.messages().await;
        // 从 messages 推导 model（取最后一个 assistant message 的 model）
        let model = msgs.iter().rev()
            .find_map(|m| match m {
                AgentMessage::Assistant(a) => Some(a.model.clone()),
                _ => None,
            }).unwrap_or_default();
        // 分类计数
        // ...
        SessionInfo { id: meta.id, model, cwd: meta.cwd, ... }
    }
}
```

### TreeCommand 与 `get_branch()`

`Session` 已有 `get_branch()` 方法（等价于原参考中的 `get_path_to_root()`）。
可直接使用：

```rust
let entries = self.session.get_branch(None).await?;
```

### ListSessionsCommand

可复用 `main.rs` 中已实现的 `format_session_list()` 逻辑，将其移到 session 模块或 format 层。

### ListSessionsCommand

扫描 `~/.pi/agent/sessions/` 目录，解析 JSONL header，用 `OutputFormatter::session_list()` 展示。

```rust
impl Command for ListSessionsCommand {
    fn execute(&self, _args: &[&str]) -> Result<()> {
        let sessions = self.scanner.list_sessions()?;
        let output = self.formatter.session_list(&sessions);
        println!("{}", output);
        Ok(())
    }
}
```

### 可测试性

- SessionCommand：用 InMemorySession 注入已知数据，断言输出字符串包含特定字段
- TreeCommand：构建已知的 entry 链，断言树形缩进格式
- ListSessionsCommand：用空目录/有 session 的目录分别测试

## 测试

- `/session` → 输出包含 session id
- `/tree` → 输出包含缩进结构
- `/list-sessions` → 空目录输出提示；有文件时输出表格
- `/list-sessions` 列顺序（按时间倒序）

## 文件改动

| 文件 | 改动 |
|---|---|
| `src/coding_agent/command.rs` | 添加 SessionCommand、TreeCommand、ListSessionsCommand |
| `src/format/out.rs` | 添加 `session_list()`、`session_tree()` |
| `src/agent/session/session.rs` | 可能需要暴露 `get_path_to_root()`（如果还没公开） |

## Blocked by

Ticket 19（format module）、Ticket 20（CommandRegistry）
