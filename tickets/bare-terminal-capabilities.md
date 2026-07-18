# 裸终端（Bare Terminal）能力摸底

## 什么是裸终端

不引入 TUI 框架（ratatui、cursive 等），直接用 ANSI escape codes + crossterm 与终端交互。输出驻留在 scrollback 中（而非备用屏幕缓冲区），用完后可以滚动查看历史。

**代表工具：** fzf、gh CLI、bat、ripgrep、starship、zoxide

---

## 裸终端能做什么

### ✅ 全部 100% 可测

| 能力 | 实现方式 | Rust crate | 断言方式 |
|---|---|---|---|
| **彩色文本输出** | ANSI escape codes | `crossterm::style` / `colored` / `anstyle` | `assert_eq!(output, "\x1b[31mred\x1b[0m")` |
| **格式化表格** | 字符排版 | `sparcli::Table` | `.print_to(&mut buf)` → assert buf |
| **面板/边框** | 边框字符 | `sparcli::Panel` | 同上 |
| **Tree 树形** | 缩进+连线符 | `sparcli::Tree` 或手写 | 同上 |
| **Diff 着色** | 行比较+颜色 | `similar` + 颜色包裹 | assert 含颜色的字符串 |
| **Key-Value 列表** | 对齐打印 | `sparcli` 或手写 | assert 字符串 |
| **进度/状态行** | 单行覆盖 | `indicatif` / 手写 `\r` | 可断言最终行 |
| **语法高亮** | syntect -> ANSI | `syntect` + `crossterm` | assert 含 escape 的字符串 |
| **Markdown 渲染** | 解析+打印 | `termimad` | assert 输出 |
| **选择器 Select** | 临时全屏覆盖 | `inquire::Select` / `sparcli::Select` | 选择器结果可断言 |
| **确认 Confirm** | 单行 y/n | `inquire::Confirm` / `sparcli::Confirm` | assert 返回值 |
| **文本输入** | 行编辑 | `inquire::Text` / `rustyline` | assert 返回值 |
| **分页器 Pager** | 逐屏输出 | `sparcli::Pager` / `less -F` | 非交互时全输出 |

### ⚠️ 能做但需要手写

| 能力 | 工作量 | 说明 |
|---|---|---|
| **交互式 tree 导航** | ~300 行 | 需要 raw mode + 事件循环 + 状态管理 |
| **实时 streaming 输出** | ~100 行 | 逐 chunk 打印，我们已有 |
| **当前行覆盖更新** | ~50 行 | `\r` + `clear_line` 即可 |
| **选择器弹窗** | 委托给 inquire | inquire 自己处理渲染和恢复 |

### ❌ 不能做（或做得不好）

| 能力 | 原因 |
|---|---|
| **分屏布局** | 没有框架管理区域分割，手写非常复杂 |
| **Resize 自适应** | 需要 SIGWINCH 处理+重排，裸终端无此抽象 |
| **覆盖层 Popup** | 需要保存/恢复屏幕区域，裸终端做起来很累 |
| **实时多面板更新** | 无法做到一个面板动另一个不动 |
| **Mouse 交互** | 可以捕捉 mouse event，但没有组件树转发 |

---

## Crate 生态全景

### 输出格式化

| Crate | 能力 | 可测性 |
|---|---|---|
| **sparcli** | Table, Panel, Tree, Diff, List, KV, Alert, Badge, Spinner, Progress, Pager | ✅ `print_to(&mut writer)` 捕获输出 |
| **termimad** | Markdown → 终端渲染（含表格、列表、代码块、链接） | ✅ `termimad::print_text()` → 可重定向 |
| **syntect** | 语法高亮 → ANSI 字符串 | ✅ `syntect::ansi()` → String |
| **colored / anstyle** | 颜色包装 | ✅ String |
| **indicatif** | 进度条/Spinner | ✅ `ProgressBar::suspend()` 测试模式 |

### 交互输入

| Crate | 能力 | 可测性 |
|---|---|---|
| **inquire** | Select, MultiSelect, Text, Confirm, Password, Date, Editor, Autocomplete | ✅ 返回 `Result<T, InquireError>` |
| **sparcli**（输入部分） | Confirm, Text, Select, MultiSelect, Textarea, FuzzySelect, DatePicker | ✅ 返回 `Outcome<T>` |
| **requestty** | Select, Input, Confirm, Password, RawSelect, Expand, Checkbox | ✅ 基于 Future |
| **dialoguer** | Select, Input, Confirm, Password, Sort, FuzzySelect | ✅ 返回 `Result<T>` |

### 行编辑/REPL

| Crate | 能力 | 可测性 |
|---|---|---|
| **rustyline** | readline 风格：历史、搜索、vi/emacs 模式、补全 | ✅ 可注入 `Completer`/`Hinter` 测试 |
| **reedline** | Nushell 的行编辑，支持提示、高亮、多行 | ✅ `Reedline::create_with_backend()` |

### 终端底层

| Crate | 能力 |
|---|---|
| **crossterm** | 光标、颜色、raw mode、事件、尺寸 |
| **termion** | 纯 Rust 终端控制（Unix only） |

### 关键发现：sparcli

`sparcli` 是裸终端路线最关键的发现。它：
1. **不依赖 ratatui**，直接用 crossterm 渲染
2. 每个输出组件实现 `Renderable` trait：`.print()` 直接输出、`.print_to(&mut writer)` 捕获到 buffer
3. 已有 Table、Panel、Tree、Diff、List、KV、Alert、Badge 等组件
4. 输入部分有 Confirm、Text、Select、MultiSelect、Textarea、FuzzySelect
5. 统一主题，pipe-aware，NO_COLOR 支持
6. 无 panic，所有错误走 Result

```rust
// 测试写法
let mut buf = Vec::new();
Table::new().columns(["Name", "Status"])
    .row(["agent", "running"])
    .row(["db", "online"])
    .striped(true)
    .print_to(&mut buf)?;
assert!(String::from_utf8_lossy(&buf).contains("agent"));
```

---

## 裸终端 vs TUI 能力对照

| 场景 | 裸终端 | ratatui TUI | 代码量差 |
|---|---|---|---|
| `> prompt` 输入 + streaming 回复 | ✅ 已经在做 | ✅ 一样 | 相同 |
| `/model` 选模型 | ✅ `inquire::Select` 弹出→选完消失（5 行） | ✅ 自定义组件（~100 行） | 裸终端省 95% |
| `/session` 看信息 | ✅ 打印彩色 KV 列表（10 行） | ✅ 自定义组件（~50 行） | 裸终端省 80% |
| `/tree` 打印 session 树 | ✅ 打印缩进文本（50 行） | ✅ 交互式组件（~300 行） | 裸终端省 83% |
| 实时 streaming | ✅ 逐 chunk 打印（已实现） | ✅ Terminal::draw 每帧重绘 | 相同 |
| Streaming 中弹出 selector | ❌ 不能 | ✅ 可以 overlay | TUI 胜 |
| 分屏：左边 tree 右边对话 | ❌ 做不到 | ✅ 可以 | TUI 胜 |
| Resize 窗口适配 | ❌ 不处理 | ✅ 自动重排 | TUI 胜 |
| 全屏沉浸感 | ❌ scrollback 模式 | ✅ alternate screen | TUI 胜 |

---

## 结论

**裸终端在我们的"100% 可测"约束下，能覆盖 80% 的用户场景。** 缺失的部分是"实时重绘"和"分屏布局"——这些只有 TUI 能做，但它们对 coding agent 的日常使用不是必需的。

可以定一个简单的分界原则：
1. **能用 inquire/sparcli 一行弹窗解决的** → 裸终端做
2. **需要打印格式化信息的** → 裸终端做（sparcli 的 `print_to` 可捕获测试）
3. **需要实时多面板/分屏/覆盖层的** → 以后换 ratatui，代码量不大
