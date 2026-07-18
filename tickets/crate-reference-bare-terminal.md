# Crate 参考索引 — 裸终端交互栈

> 给新 agent 看的 API 速查 + 测试模式 + 参考链接
> 不代替官方文档，只覆盖我们在 rusty-pi 中实际使用的子集。

---

## 1. sparcli — 格式化输出（Table、Panel、Tree、KV、Diff…）

### 用途

代替手写 ANSI escape codes，输出格式化表格、面板、树、键值列表、告警、Diff 等。

### 关键 API

```rust
use sparcli::prelude::*;

// ── Alert ──
Alert::success("Build finished.").print()?;
Alert::error("Something failed.").print()?;
Alert::warning("Disk space low.").print()?;
Alert::info("Processing...").print()?;

// ── Table ──
Table::new()
    .columns(["Name", "Status"])
    .row(["agent", "running"])
    .row(["db-1", "online"])
    .striped(true)
    .title(Title::new("Services"))
    .print()?;

// ── Panel（带边框） ──
Panel::new("All systems nominal.")
    .title(Title::new("Status"))
    .print()?;
// 输出：
// ╭─ Status ─────────────╮
// │ All systems nominal. │
// ╰──────────────────────╯

// ── Key-Value List ──
KeyValueList::new()
    .entry("Session", "abc123")
    .entry("Model", "deepseek-v4-flash")
    .entry("Messages", "3")
    .entry("CWD", "/project")
    .print()?;

// ── Tree（带缩进连线） ──
Tree::new()
    .entry("root")
    .entry(TreeEntry::new("child1").indent(1))
    .entry(TreeEntry::new("child2").indent(1))
    .print()?;

// ── Badge（标签） ──
Badge::new("success", Color::Green).print()?;

// ── Spinner（动画） ──
let spinner = Spinner::new().message("Thinking...");
// spinner 自动在所在行做覆盖动画
spinner.finish_with_message("Done!");

// ── Diff ──
let diff = Diff::new()
    .left("old content")
    .right("new content")
    .language("rust");        // 选填，语法高亮
diff.print()?;
```

### 测试模式

所有 `Renderable` 组件都有 `print_to(&mut dyn Write)`：

```rust
#[test]
fn test_table_output() {
    let mut buf = Vec::new();
    Table::new()
        .columns(["Name", "Status"])
        .row(["agent", "running"])
        .striped(true)
        .print_to(&mut buf)
        .unwrap();
    let output = String::from_utf8_lossy(&buf);
    assert!(output.contains("agent"));
    assert!(output.contains("running"));
}

#[test]
fn test_kv_output() {
    let mut buf = Vec::new();
    KeyValueList::new()
        .entry("Model", "deepseek-v4-flash")
        .print_to(&mut buf)
        .unwrap();
    let output = String::from_utf8_lossy(&buf);
    assert!(output.contains("deepseek-v4-flash"));
}
```

### 主题

```rust
let theme = Theme::default()
    .accent(Color::Cyan)      // 强调色
    .muted(Color::DarkGrey)   // 次要文本颜色
    .danger(Color::Red);      // 错误色
// 传给 OutputFormatter 统一使用
```

### 参考链接

- docs.rs: <https://docs.rs/sparcli/latest/sparcli/>
- crates.io: <https://crates.io/crates/sparcli>
- GitHub: <https://github.com/cgroening/rs-sparcli>

> **版本说明：** 方案最初写 0.2，实际 crates.io 最新为 **0.3.0**。已验证 0.3.0 API 与本文档完全兼容。使用 `sparcli = "0.3"`。

---

## 2. inquire — 交互选择器

### 用途

终端选择器弹窗：选模型、选 session、输入文本、确认。

### 关键 API

```rust
use inquire::{Select, Text, Confirm, MultiSelect};

// ── Select（单选） ──
let model = Select::new(
    "Select model:",
    vec!["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-coder-v2"],
).prompt()?;
// 类型：Result<&str, InquireError>
// 测试：直接 assert_eq!(model.unwrap(), "deepseek-v4-flash")

// ── 带自定义类型的 Select ──
#[derive(Debug, Clone, Display)]
struct ModelOption {
    id: &'static str,
    label: &'static str,
}
impl std::fmt::Display for ModelOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.label, self.id)
    }
}
let options = vec![
    ModelOption { id: "df-v4-flash", label: "DeepSeek Flash" },
    ModelOption { id: "df-v4-pro", label: "DeepSeek Pro" },
];
let selected = Select::new("Select model:", options).prompt()?;
// selected: ModelOption

// ── Text（文本输入） ──
let name = Text::new("Session name:")
    .with_default("default")
    .with_help_message("Optional session name")
    .prompt()?;

// ── Confirm（是/否） ──
let proceed = Confirm::new("Continue?")
    .with_default(true)
    .prompt()?;

// ── MultiSelect（多选） ──
let selected = MultiSelect::new("Select tools:",
    vec!["bash", "read", "write", "edit", "find", "grep", "ls"],
).prompt()?;

// ── 自定义过滤（FuzzySelect 风格） ──
// Select 默认支持打字过滤，光标移动到匹配项
```

### 测试模式

`inquire` 的测试很简单——直接 mock 返回结果：

```rust
#[test]
fn test_model_selection() {
    // 不需要真正弹窗口测试 inquire 本身
    // 只需要测试选择后的处理逻辑
    let selected = "deepseek-v4-flash".to_string();
    let result = handle_model_change(&selected);
    assert_eq!(result, ModelId("deepseek-v4-flash"));
}

#[test]
fn test_selection_cancelled() {
    let result = handle_model_change(None);
    assert_eq!(result, None); // 用户取消，不变
}
```

对于需要测试 inquire 集成路径的函数，可以重构为接收 `Fn()` 参数：

```rust
fn show_model_picker<F>(picker: F) -> Result<String>
where F: Fn() -> Result<String, InquireError>
{
    picker()
}

// 生产环境传真实 inquire
show_model_picker(|| {
    Select::new("Model:", vec!["a".into(), "b".into()])
        .prompt()
        .map(|s| s.to_string())
})

// 测试环境传 mock
show_model_picker(|| Ok("a".to_string()))
```

### 参考链接

- docs.rs: <https://docs.rs/inquire/latest/inquire/>
- crates.io: <https://crates.io/crates/inquire>
- GitHub: <https://github.com/mikaelmello/inquire>

> **版本说明：** 方案最初写 0.7，实际 crates.io 最新为 **0.9.4**。已验证 0.9.4 API 与本文档完全兼容。使用 `inquire = "0.9"`。

---

## 3. rustyline — REPL 行编辑

### 用途

替代 `std::io::stdin().read_line()`，提供：
- 命令历史（上下箭头）
- 行内编辑（vi/emacs 模式）
- Ctrl+R 搜索历史
- Tab 补全
- 历史持久化到文件

### 关键 API

```rust
use rustyline::{DefaultEditor, Result};

let mut rl = DefaultEditor::new()?;

// 加载历史（跨 session 持久化）
let history_path = dirs::data_dir()
    .unwrap_or_else(|| PathBuf::from("."))
    .join("rusty-pi").join("history.txt");
if rl.load_history(&history_path).is_err() {
    // 首次运行，无历史文件
}

loop {
    let line = rl.readline("> ")?;
    rl.add_history_entry(line.as_str())?;
    // 处理 line...
}

// 退出前保存
rl.save_history(&history_path)?;
```

### 测试模式

rustyline 默认从 stdin 读，测试需要注入输入。有两种方式：

**方式 A：自定义 `Completer` + 模拟 stdin**（复杂）

**方式 B（推荐）：把 rustyline 放到一个 trait 后面**

```rust
pub trait LineReader {
    fn readline(&mut self, prompt: &str) -> Result<String>;
    fn add_history(&mut self, line: &str);
    fn save_history(&mut self) -> Result<()>;
}

// 生产：RealLineReader(DefaultEditor)
// 测试：MockLineReader { lines: Vec<String>, idx: usize }

pub struct MockLineReader {
    pub lines: Vec<String>,
    pub history: Vec<String>,
    idx: usize,
}

impl LineReader for MockLineReader {
    fn readline(&mut self, _prompt: &str) -> Result<String> {
        if self.idx < self.lines.len() {
            let line = self.lines[self.idx].clone();
            self.idx += 1;
            Ok(line)
        } else {
            Err(rustyline::error::ReadlineError::Eof)
        }
    }
    fn add_history(&mut self, line: &str) {
        self.history.push(line.to_string());
    }
    fn save_history(&mut self) -> Result<()> { Ok(()) }
}

#[test]
fn test_repl_prompts() {
    let mut reader = MockLineReader {
        lines: vec!["hello".into(), "/exit".into()],
        history: vec![],
        idx: 0,
    };
    // 把 reader 注入 REPL，验证行为
}
```

### 参考链接

- docs.rs: <https://docs.rs/rustyline/latest/rustyline/>
- crates.io: <https://crates.io/crates/rustyline>
- GitHub: <https://github.com/kkawakam/rustyline>

---

## 4. crossterm — 终端底层控制

> 注意：sparcli 和 inquire 已经封装了大部分 crossterm 功能，我们通常不需要直接调用。
> 只有少数场景需要：清屏、查终端尺寸、raw mode。

### 关键 API

```rust
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::execute;

// 清屏
execute!(stdout(), Clear(ClearType::All))?;

// 查终端尺寸
let (cols, rows) = terminal::size()?;

// 进入/退出 raw mode（通常由 inquire/rustyline 自动管理）
terminal::enable_raw_mode()?;
// ... 交互 ...
terminal::disable_raw_mode()?;

// 光标操作
use crossterm::cursor::{MoveTo, Show, Hide};
execute!(stdout(), MoveTo(0, 0))?;
execute!(stdout(), Hide)?;
execute!(stdout(), Show)?;
```

### 测试模式

crossterm 的测试通常 mock 掉。把 `execute!` 调用包装到接口后面：

```rust
pub trait TerminalOps {
    fn clear_screen(&self) -> Result<()>;
    fn terminal_size(&self) -> Result<(u16, u16)>;
}

pub struct RealTerminal;
impl TerminalOps for RealTerminal { /* 调用 crossterm */ }

pub struct MockTerminal {
    pub size: (u16, u16),
}
impl TerminalOps for MockTerminal {
    fn clear_screen(&self) -> Result<()> { Ok(()) }
    fn terminal_size(&self) -> Result<(u16, u16)> { Ok(self.size) }
}
```

### 参考链接

- docs.rs: <https://docs.rs/crossterm/latest/crossterm/>
- crates.io: <https://crates.io/crates/crossterm>
- GitHub: <https://github.com/crossterm-rs/crossterm>

---

## 5. Optional: termimad — Markdown 终端渲染

### 用途

将 LLM 回复中的 Markdown 格式化为带颜色的终端文本（代码块、标题、列表、表格等）。

### 关键 API

```rust
// 最简单用法
termimad::print_text("**bold** and *italic* and `code`");

// 自定义皮肤
let skin = termimad::MadSkin::default();
skin.print_text("# Hello\nThis is **markdown**");

// 捕获到字符串（测试用）
let mut buf = Vec::new();
skin.write_text("`code`", &mut buf)?;
let output = String::from_utf8_lossy(&buf);
```

### 参考链接

- docs.rs: <https://docs.rs/termimad/latest/termimad/>
- crates.io: <https://crates.io/crates/termimad>
- GitHub: <https://github.com/Canop/termimad>

---

## 6. Optional: syntect — 语法高亮

### 用途

对代码片段做语法高亮，输出 ANSI 字符串。可用于 `bat` 风格的代码预览、Diff 着色增强。

### 关键 API

```rust
use syntect::parsing::SyntaxSet;
use syntect::highlighting::ThemeSet;
use syntect::easy::HighlightLines;

let ss = SyntaxSet::load_defaults_newlines();
let ts = ThemeSet::load_defaults();
let syntax = ss.find_syntax_by_extension("rs").unwrap();
let mut h = HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);
for line in code.lines() {
    let ranges = h.highlight_line(line, &ss).unwrap();
    let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
    print!("{}", escaped);
}
```

### 参考链接

- docs.rs: <https://docs.rs/syntect/latest/syntect/>
- crates.io: <https://crates.io/crates/syntect>
- GitHub: <https://github.com/trishume/syntect>

---

## 7. Exa MCP 搜索引擎

### 用途

搜索外网技术资料（Rust crate 文档、最佳实践、替代方案调研）。

### 问题背景

Node.js v22 原生 `fetch`（基于 undici）**不读取** `http_proxy`/`https_proxy` 环境变量。
在有代理（Clash）的环境下直接运行 `mcporter call 'exa.web_search_exa(...)'` 会报 `ENETUNREACH`。

### 解决方案

用 undici 的 `ProxyAgent` + `setGlobalDispatcher` 替换 Node.js 全局 HTTP 调度器：

**一次性安装：**

```bash
cd /tmp && npm install undici
```

**代理注入脚本（已创建，路径勿改）：**

文件：`/tmp/proxy_hook2.cjs`

```javascript
const { ProxyAgent, setGlobalDispatcher } = require('undici');
const proxyUrl = process.env.GLOBAL_AGENT_HTTP_PROXY || process.env.HTTPS_PROXY;
if (proxyUrl) {
  setGlobalDispatcher(new ProxyAgent(proxyUrl));
  console.error('[proxy] using', proxyUrl);
}
```

**使用方式：**

```bash
# 搜索（加 --require 注入代理）
NODE_OPTIONS="--require /tmp/proxy_hook2.cjs" \
GLOBAL_AGENT_HTTP_PROXY=http://127.0.0.1:7897 \
mcporter call 'exa.web_search_exa(query: "ratatui TestBackend insta snapshot", numResults: 5)'

# 列出可用工具
NODE_OPTIONS="--require /tmp/proxy_hook2.cjs" \
GLOBAL_AGENT_HTTP_PROXY=http://127.0.0.1:7897 \
mcporter list exa --schema
```

### 验证代理是否工作

```bash
curl -s --connect-timeout 10 "https://mcp.exa.ai/mcp"
# 应返回 JSON-RPC 错误响应（"Method not allowed"）说明可达
```

### Debug 提示

如果 `mcporter call` 仍然 `ENETUNREACH`：
- 确认 Clash 代理在运行：`curl -x http://127.0.0.1:7897 https://www.google.com`
- 确认 `GLOBAL_AGENT_HTTP_PROXY` 设置正确
- 检查 `/tmp/node_modules/undici/` 存在
- curl 能通则 mcporter 不能通=代理注入没生效，检查 `NODE_OPTIONS` 拼写

### 参考链接

- Exa MCP: <https://mcp.exa.ai/mcp>
- undici ProxyAgent: <https://github.com/nodejs/undici#proxyagent>
- mcporter: 项目内 `~/.mcporter/mcporter.json` 配置

---

## 8. 参考对照表：sparcli vs ratatui

| 概念 | ratatui | sparcli（裸终端） |
|---|---|---|
| 渲染模式 | 每帧全量重绘到 buffer | 直接写 stdout/stderr |
| 测试方式 | `TestBackend` → buffer 断言 | `print_to(&mut buf)` → String 断言 |
| 布局 | Layout + Constraint | 无布局，按打印顺序 |
| 组件 | Widget trait | Renderable trait |
| 事件循环 | 手写 | 不需要（inquire/rustyline 自带） |
| 状态管理 | 手写 | 不需要（选择器的结果直接返回） |
| 测试脆弱性 | snapshot 更新成本 | 低（纯字符串相等） |
| 迁移到 ratatui | — | 输出已经是 String，ratatui 可消费 |
