# Ticket 17 — Context Files 支持

## 现状

- `system_prompt.rs` 的 `BuildSystemPromptOptions` 已有 `context_files: Vec<ContextFile>` 和对应的处理逻辑
- 但 CLI 没有 `-c`/`--context` 参数暴露这个功能
- 每次只能通过 `--prompt-path` 指定模板或直接输入 prompt，无法将项目文件内容注入 system prompt

## 目标

```bash
# 启动时注入一个或多个文件内容到 system prompt
rusty-pi -p deepseek -c src/main.rs "explain this file"
rusty-pi -c AGENTS.md -c SPEC.md "what should I work on next?"
```

## Blocked by

None（~200 行，大部分是 CLI 参数胶水）

## 设计要点

### 1. CLI 参数

```rust
/// Path to context file(s) whose content is injected into the system prompt
#[arg(short = 'c', long = "context")]
context: Vec<PathBuf>,
```

### 2. ContextFile 结构

`system_prompt.rs` 中已定义：

```rust
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}
```

CLI 参数中的路径在加载时读取内容，构造 `ContextFile`。

### 3. 读取时机

在 `main.rs` 中，clap 解析后立即读取 context 文件：

```rust
let context_files: Vec<ContextFile> = cli.context.iter()
    .map(|p| {
        let resolved = if p.is_relative() { cwd.join(p) } else { p.clone() };
        let content = std::fs::read_to_string(&resolved)
            .map_err(|e| anyhow::anyhow!("Cannot read context file {}: {}", resolved.display(), e))?;
        Ok(ContextFile { path: resolved, content })
    })
    .collect::<anyhow::Result<_>>()?;
```

### 4. 传入 system prompt

将 `context_files` 传入 `build_system_prompt` 的 `BuildSystemPromptOptions::context_files` 字段。

### 5. 格式

参考原版，context 文件在 system prompt 中的格式：

```
<context_file path="src/main.rs">
// file content here
</context_file>
```

当前 `system_prompt.rs` 中的 `format_context_files` 函数（如果已实现）使用此格式。

### 6. 错误处理

- 文件不存在 → 打印错误并跳过（不影响 session 启动）
- 文件过大（> 1MB）→ 警告并截断
- 二进制文件 → 警告并跳过

## 测试策略

- CLI 参数解析：`-c file1 -c file2`
- context 文件内容出现在 system prompt 中
- context 文件不存在的处理
- 空 context 列表不改变 system prompt

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `src/main.rs` | 添加 `-c`/`--context` 参数；读取文件内容；传入 `build_system_prompt` |
| `src/coding_agent/prompt_session.rs` | 在 `new()` 中接受 context files 并传给 system prompt builder |
