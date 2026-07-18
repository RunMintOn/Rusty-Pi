# Spec: 裸终端交互架构

## 目标

在不引入 ratatui 的前提下，给 rusty-pi 提供一套可交互、可格式化输出、100% 可自动化测试的终端交互层。

---

## Crate 选型

| 用途 | Crate | 版本 | 理由 |
|---|---|---|---|
| 终端控制（颜色、光标、raw mode） | `crossterm` | 0.28+ | 已通过 rustyline 间接依赖；跨平台 |
| 格式化输出（Table, Panel, Tree, Diff, List, KV, Alert） | `sparcli` | 0.3+ | 裸终端专用，`print_to(&mut buf)` 捕获输出用于测试；无 ratatui 依赖 |
| 交互选择器（Select, MultiSelect, Text, Confirm） | `inquire` | 0.9+ | 返回 `Result<T>`，测试只需 assert 返回值 |
| REPL 行编辑（历史、搜索、vi 模式） | `rustyline` | 15+ | 原本 ticket 18 已选 |
| Markdown → 终端渲染（可选） | `termimad` | 0.31+ | 需要时再加 |
| 语法高亮（可选） | `syntect` | 5+ | 需要时再加 |

**选 sparcli 而非手写 ANSI 的理由：**
- 已有 Table、Panel、Tree、Diff、Alert、KV、Badge、Spinner 等开箱即用
- `print_to(&mut buf)` 是所有输出组件的标准接口，测试零成本
- 尊重 `NO_COLOR`，pipe 自动去色，这些东西手写容易漏
- 和将来换 ratatui 不冲突——sparcli 的输出是 `String`，ratatui 也能消费

---

## 分层架构

```
┌──────────────────────────────────────────────────┐
│  Layer 3: 交互层（src/interact/）                  │
│  repl.rs → rustyline + CommandRegistry             │
│  prompts.rs → inquire 包装                         │
│  测试：注入 mock Agent，断言 stdout 文本            │
├──────────────────────────────────────────────────┤
│  Layer 2: 格式化层（src/format/）                   │
│  out.rs → sparcli 组件 + 自定义格式化器              │
│  diff.rs → similar + 颜色                          │
│  markdown.rs → termimad（可选）                     │
│  测试：.print_to(&mut buf) → assert_eq!(got, expected) │
├──────────────────────────────────────────────────┤
│  Layer 1: 核心层（src/agent/ + src/ai/）            │
│  不变，保持纯逻辑，不 import 任何 UI crate            │
│  测试：原有的 200+ 测试                              │
└──────────────────────────────────────────────────┘
```

### 关键规则

**Layer 1 不引用 Layer 2 和 Layer 3。** `Agent` 不知道你在用 sparcli 还是 ratatui。它的输入输出都是纯 `String`/`AgentMessage`。

**Layer 2 只引用 Layer 1 和 sparcli。** 格式化函数接收数据、返回格式化的 String（或直接写 writer）。

**Layer 3 引用 Layer 1 和 Layer 2。** REPL 用 rustyline 读输入、调用 Agent、用 Layer 2 格式化输出。

---

## Layer 3：交互层设计

### REPL 循环（repl.rs）

```
loop {
    line = rustyline.readline("> ")               // 读输入（带历史+行编辑）
    if line 以 / 开头 → command_registry.dispatch()  // 斜杠命令
    else → agent.run(line)                         // 普通 prompt
    while agent 正在运行 {
        chunk = stream.next()                      // 从 streaming 拿输出
        print!("{}", chunk)                        // 直接输出到终端
        stdout.flush()
    }
}
```

### CommandRegistry

```rust
pub struct CommandRegistry {
    commands: Vec<Box<dyn Command>>,
}

impl CommandRegistry {
    pub fn register(&mut self, cmd: Box<dyn Command>);
    pub fn dispatch(&self, input: &str) -> Result<()>;
}
```

默认命令：

| 命令 | 行为 | 实现方式 |
|---|---|---|
| `/help` | 列出所有命令 | 打印 command.name + description |
| `/exit` / `/quit` | 退出 REPL | 已有 |
| `/session` | 显示当前 session 信息 | 打印 KV 列表（sparcli） |
| `/model` | 切换模型 | inquire::Select 弹出 |
| `/context` | 查看/注入 context files | 打印列表 + inquire::Text |
| `/list-sessions` | 列出已保存 session | 打印 Table（sparcli） |
| `/tree` | 打印 session 树 | 打印缩进文本（手写，sparcli::Tree 可选） |
| `/clear` | 清屏 | crossterm::terminal::clear |
| `/compact` | 触发 compaction | 调用 Agent |

### 选择器模式（临时覆盖）

```
> /model
? Select model:                                    ← inquire::Select 接管终端
  ▸ deepseek-v4-flash
    deepseek-v4-pro
    deepseek-coder-v2
✓ Selected: deepseek-v4-pro                        ← 选择完成，结果行留在 scrollback
>                                                    ← 回到 REPL
```

inquire 自己在执行期间处理：
- raw mode 设置/恢复
- 光标管理
- 终端恢复

选择完成后的结果行由我们打印到 scrollback，作为"发生了什么"的记录。

---

## Layer 2：格式化层设计

### OutputFormatter

```rust
pub struct OutputFormatter {
    theme: Theme,   // sparcli 的 Theme
}

impl OutputFormatter {
    /// Session 信息 → 彩色 KV 列表
    pub fn session_info(&self, info: SessionInfo) -> String {
        // 用 sparcli::Table 或手写对齐
        let mut buf = Vec::new();
        KeyValueList::new()
            .entry("Session", info.id)
            .entry("Model", info.model)
            .entry("Messages", &info.msg_count.to_string())
            .entry("CWD", info.cwd)
            .print_to(&mut buf)?;
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Session 树 → 缩进文本
    pub fn session_tree(&self, entries: &[SessionTreeEntry]) -> String { ... }

    /// Model 列表 → Table
    pub fn model_list(&self, models: &[ModelInfo]) -> String {
        let mut buf = Vec::new();
        Table::new()
            .columns(["Provider", "Model", "Status"])
            .rows(models.iter().map(|m| [m.provider, m.id, m.status]))
            .print_to(&mut buf)?;
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Session 列表 → Table
    pub fn session_list(&self, sessions: &[SessionSummary]) -> String { ... }
}
```

### 测试模式

```rust
#[test]
fn test_session_info_output() {
    let fmt = OutputFormatter::default();
    let info = SessionInfo {
        id: "abc123".into(),
        model: "deepseek-v4-flash".into(),
        msg_count: 3,
        cwd: "/project".into(),
    };
    let output = fmt.session_info(&info);
    assert!(output.contains("abc123"));
    assert!(output.contains("deepseek-v4-flash"));
    assert!(output.contains("/project"));
}
```

---

## 组件依赖关系

```
Cargo.toml 新增依赖：
  sparcli = "0.3"      # 格式化输出（已验证 0.3.0 API 兼容）
  inquire = "0.9"      # 交互选择器（已验证 0.9.4 API 兼容）
  rustyline = "15"      # 行编辑（已有计划）
  
可选（以后再加）：
  termimad = "0.31"    # Markdown 渲染
  syntect = "5"        # 语法高亮
```

---

## 渐进式交付路线

不一次做完所有事情。按现有 ticket 顺序，逐步替换：

| 步骤 | 改动 | 涉及文件 |
|---|---|---|
| 1. 加 sparcli + inquire 依赖 | Cargo.toml | Cargo.toml |
| 2. 创建 `src/format/` 模块 | 搬移格式化相关逻辑 | `src/format/mod.rs`, `src/format/out.rs` |
| 3. `OutputFormatter` 基本版 | session info、model list、error | `src/format/out.rs` |
| 4. REPL 接入 CommandRegistry | 斜杠命令系统 | `src/coding_agent/repl.rs` |
| 5. `/model` → inquire::Select | 模型选择交互 | `src/coding_agent/` |
| 6. `/session` → 格式化输出 | session 信息展示 | `src/coding_agent/` |
| 7. 逐步替换其余命令 | /help、/context、/tree、/list-sessions | 各命令文件 |

---

## 已知缺口（评估发现，需在对应 ticket 中补充）

### 1. `ProviderApi` 缺少模型列表与运行时切换（对应 Ticket 21）

当前 trait 只有 `stream()` 方法，没有暴露模型列表或切换模型的能力。
`/model` 命令需要：

- 给 `ProviderApi` 加 `fn list_models(&self) -> Vec<&Model>`
- 给 `Agent` 加 `fn switch_model(&mut self, model: Model)`
- 给 `PromptSession` 加 `fn switch_model()` 包装

### 2. `Agent` 缺少 `on_tool_start` / `on_tool_end` 回调（对应 Ticket 23）

当前只有 `on_text` 一个 callback。工具执行发生在 `Agent::execute_tool()` 私有方法内，
外部无法感知工具开始、结束、耗时。需要：

- 加 `on_tool_start: Option<ToolStartCallback>` 字段
- 加 `on_tool_end: Option<ToolEndCallback>` 字段
- 在 `execute_tool()` 的前后调用

### 3. `SessionInfo` 结构体与提取方法不存在（对应 Ticket 22）

`/session` 需要展示 `{id, model, msg_count, cwd}`。当前 `Session::get_metadata()` 返回
`SessionMetadata { id, created_at, cwd, ... }`——没有 `model`，没有现成的 `msg_count`。
需要：

- 定义 `SessionInfo` 结构体
- 加 `Session::get_info()` 方法，从 session messages/entries 中推导 model 和计数
- 借用 `derive_session_context_state()` 已有的推导逻辑
