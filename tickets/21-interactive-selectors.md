# Ticket 21 — 模型选择器 + 上下文注入（inquire 交互命令）

## 现状

- `/model` 命令不存在（只能启动时通过 `-p deepseek -m model` 指定）
- 运行时无法切换模型
- `/context` 命令不存在（只能启动时通过 `-c file` 注入）

## 目标

实现两条斜杠命令，使用 inquire 做交互选择：

```
> /model
? Select model:
  ▸ deepseek-v4-flash
    deepseek-v4-pro
    deepseek-coder-v2
✓ Switched to deepseek-v4-pro

> /context src/main.rs
✓ Added src/main.rs (2.1KB) to system prompt
```

## 设计

参考 `crate-reference-bare-terminal.md` 第 2 节（inquire）。

### ModelCommand

```rust
impl Command for ModelCommand {
    fn name(&self) -> &str { "model" }
    fn description(&self) -> &str { "Switch model (interactive selector)" }

    fn execute(&self, _args: &[&str]) -> Result<()> {
        let models = self.provider.list_models();
        let selected = Select::new("Select model:", models).prompt()?;
        self.session.switch_model(selected)?;
        println!("✓ Switched to {}", selected);
        Ok(())
    }
}
```

### 前提条件：ProviderApi 需要补充模型列表方法

当前 `ProviderApi` trait（`src/ai/providers/mod.rs`）只有 `stream()` 方法，没有暴露模型列表。
执行本 ticket 前必须先完成：

1. 给 `ProviderApi` 加 `fn list_models(&self) -> Vec<&Model>`
2. 给 `Agent` 加 `fn switch_model(&mut self, model: Model)`
3. 给 `PromptSession` 加 `fn switch_model(model: Model)` 包装方法

```rust
// ProviderApi 新增
pub trait ProviderApi: Send + Sync {
    async fn stream(...) -> ...;
    fn list_models(&self) -> Vec<&Model>;  // 新增
}

// Agent 新增
impl Agent {
    pub fn switch_model(&mut self, model: Model) {
        self.model = model;
    }
}
```

> 注意：`Model` 是 `&'static str` 的引用，切换时直接替换 `Agent` 的 `model` 字段即可。
> Provider 不变，只换模型 ID。

### ModelCommand 实现

### ContextCommand

```rust
impl Command for ContextCommand {
    fn name(&self) -> &str { "context" }
    fn description(&self) -> &str { "Inject a file into system prompt" }

    fn execute(&self, args: &[&str]) -> Result<()> {
        let path = if args.is_empty() {
            Text::new("File path:").with_help_message("Path to file to inject").prompt()?;
        } else {
            args[0].to_string()
        };
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Cannot read {}: {}", path, e))?;
        self.session.add_context_file(&path, &content)?;
        let size = content.len();
        println!("✓ Added {} ({}KB) to system prompt", path, size / 1024);
        Ok(())
    }
}
```

### Pick 模式（简化版）

对于简单的选择（如切换模型），inquire::Select 直接弹窗→选完消失，结果行打印到 scrollback。

对于文本输入（如 context 文件路径），inquire::Text 提供行编辑、默认值、帮助消息。

### 可测试性

将 inquire 调用封装到 trait 后，测试时注入 mock：

```rust
pub trait Picker {
    fn select<T: Display>(&self, prompt: &str, options: Vec<T>) -> Result<T>;
    fn text(&self, prompt: &str, default: Option<&str>) -> Result<String>;
}
```

生产用 `RealPicker(inquire)`，测试用 `MockPicker`。

## 测试

- MockPicker 返回特定模型 → 验证 Agent 切换了模型
- MockPicker 取消（Err）→ 验证模型不变
- ContextCommand 成功/文件不存在/取消

## 文件改动

| 文件 | 改动 |
|---|---|
| `src/coding_agent/command.rs` | 添加 ModelCommand、ContextCommand |
| `src/coding_agent/prompt_session.rs` 或 `src/agent/engine.rs` | 暴露 `switch_model()`、`add_context_file()` |
| `src/coding_agent/picker.rs`（新建） | Picker trait + RealPicker + MockPicker |

## Blocked by

Ticket 20（CommandRegistry）
