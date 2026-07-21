# Spec: rusty-pi — Rust Rewrite of pi

## Problem Statement

pi 是一个 AI coding agent，目前是 TypeScript monorepo，依赖 Node.js 运行时。启动即占 ~168 MB RSS，安装体积 169 MB（其中 155 MB 为 node_modules）。在需要长时间运行、多 session 并存的场景下，内存开销偏高。

目标是将其移植到 Rust，在不改变用户功能体验的前提下，显著降低资源占用（预期 60-80% 内存下降），并提供一个单二进制分发（无需 Node.js 运行时）。

## Solution

用 Rust 完整重写 pi，产物为 `rusty-pi`。第一阶段（MVP）覆盖 agent 核心 loop + 两个 LLM provider（OpenAI Codex、DeepSeek）+ bash tool + REPL 模式。未来阶段逐步补齐剩余 providers、tools、TUI、扩展系统。

**设计宗旨：全权参考原版 TypeScript 实现，不自创、不臆想。** 所有架构决策、接口设计、行为细节以 `reference/earendil-works-pi/` 中的原版代码为准。

## User Stories

1. As a developer, I want to run `cargo run` and get a REPL prompt, so that I can interact with the agent
2. As a developer, I want to type a prompt at the REPL and get a real LLM response from DeepSeek, so that I can use the agent for real work
3. As a developer, I want the agent to invoke the bash tool when the LLM requests it, so that shell commands execute in my working directory
4. As a developer, I want to see bash tool output streamed back to the LLM and displayed on my terminal, so that I can follow execution progress
5. As a developer, I want the agent loop to handle multiple rounds of LLM ↔ tool calls in a single session, so that complex multi-step tasks complete automatically
6. As a developer, I want the agent to support OpenAI Codex provider (ChatGPT Plus/Pro subscription), so that I can use GPT-5.x Codex models
7. As a developer, I want the agent to support DeepSeek provider (standard OpenAI-compatible API), so that I can use DeepSeek models
8. As a developer, I want the REPL to support multiple prompts per session (not just one-shot), so that I have an ongoing conversation
9. As a developer, I want to provide API keys via environment variables (`DEEPSEEK_API_KEY`, Codex OAuth), so that credentials are not hardcoded
10. As a developer, I want the agent to stream LLM responses token-by-token to the terminal, so that I get immediate feedback
11. As a developer, I want to run `cargo test` and have all tests pass locally without network access, so that CI can run in isolation
12. As a developer, I want the agent to properly handle errors (invalid commands, API errors, aborted requests), so that failures are reported clearly

## Current Iteration: usable Ratatui TUI

The first user-visible TUI iteration is implemented in `rusty-pi/src/tui/app.rs` and
documented in [`rusty-pi/docs/tui.md`](rusty-pi/docs/tui.md). It keeps the accepted
`AgentEvent`, `ActivityState`, `RunOutcome`, `CommandResult`,
`ToolExecutionContext`, `CancellationToken`, and terminal-guard boundaries.

The TUI now provides:

- structured User, Assistant, Thinking, Tool, Error, and System transcript blocks;
- merged streaming assistant text, collapsed thinking, and independently
  expandable tools with cached arguments plus separate stdout/stderr;
- running/success/failed/timed-out/aborted tool states and orphan-tool fallback;
- follow-at-bottom scrolling, explicit browsing mode, unread count, Home, End,
  PageUp/PageDown, and selection routing;
- a UTF-8-safe multiline editor with paste, vertical movement, deletion commands,
  viewport tracking, and Unicode width-aware rendering;
- process-local prompt history with draft restoration and consecutive deduplication;
- a single, explicit running-input policy: editing is allowed, but Enter is
  unavailable until the current run completes or is cancelled;
- stable TestBackend snapshots for the deterministic layouts and PTY smoke
  coverage for multiline input, tool streams, scrolling, cancellation, exit, and
  terminal restoration;
- bounded tool streams (64 KiB each, UTF-8-safe head/tail truncation) and a
  soft 2,000-block transcript limit.

This iteration intentionally does not add a session picker, model picker, file
tree, full Markdown, diff editor, themes, mouse, images, plugins, completion,
or syntax highlighting.

## Implementation Decisions

### Project Structure

单 crate 起步，模块路径镜像原版 5 个 package：

```
rusty-pi/
├── src/
│   ├── agent/          ← Agent loop + harness + session types（镜像 packages/agent）
│   ├── ai/             ← LLM provider trait + Codex/DeepSeek 实现（镜像 packages/ai）
│   ├── coding_agent/   ← CLI + tools + REPL（镜像 packages/coding-agent）
│   ├── tui/            ← TUI（占位，镜像 packages/tui）
│   └── orchestrator/   ← 编排器（占位，镜像 packages/orchestrator）
```

未来拆 workspace 时，每个模块直接变成独立 crate。

### Async Runtime

使用 tokio（Rust 异步生态事实标准，匹配原版 Node.js async/await 模型）。

### Prompt Templates & Skills

`src/coding_agent/` 下四个模块镜像原版 `packages/coding-agent/src/core/`：

| 模块 | 原版文件 | 功能 |
|---|---|---|
| `prompt_templates.rs` | `prompt-templates.ts` | Markdown 模板加载、frontmatter 解析、bash 风格参数替换 |
| `skills.rs` | `skills.ts` | Agent Skills 发现、验证、XML 格式化 |
| `system_prompt.rs` | `system-prompt.ts` | System prompt 构建（tools/guidelines/skills/context） |
| `prompt_session.rs` | —（薄层封装） | 封装 agent + 展开逻辑，入口点为 REPL 和未来其他模式 |

加载优先级：global（`~/.pi/agent/`）→ project（`$CWD/.pi/`）→ 显式 `--prompt-path` / `--skill-path`。

### Tool 系统

镜像原版三层架构：

| 原版 TS | Rust 对应 |
|---|---|
| `Tool { name, description, parameters: TSchema }` | `trait Tool`（元数据 + serde 参数类型） |
| `AgentTool { execute, prepareArguments?, executionMode }` | `trait AgentTool: Tool`（执行逻辑） |
| `AgentToolResult<T>` | `struct AgentToolResult<T>` |
| TypeBox 运行时 schema 校验 | serde 编译期反序列化 + schemars JSON Schema 生成 |

先实现 bash tool（`BashParams { command: String, timeout: Option<u64> }`），工具接口为未来扩展预留。

### LLM Provider 系统

镜像原版架构：

- `Provider` trait：定义 `id`、`name`、`base_url`、`auth`、`models`、`api`
- 每个 provider 一个模块：`ai::providers::deepseek`、`ai::providers::openai_codex`
- 原版 DeepSeek 使用 `openai-completions` API（OpenAI 兼容），Rust 直接实现等效 HTTP 调用
- 原版 OpenAI Codex 使用 OAuth + SSE/WebSocket，Rust 使用 `reqwest` + OAuth 流程
- 提供 `MockProvider` 用于测试，返回预设响应

### Session 模型

镜像原版 JSONL session 格式：
- 树形结构（`id`/`parentId`）
- 支持 compaction、branch 等 entry 类型
- MVP 阶段先实现内存 session（不持久化到 JSONL 文件）

### CLI / REPL

- `clap` 解析 CLI 参数
- `rusty-pi`（无参数）→ 进入 REPL
- `rusty-pi "prompt"` → 单次 prompt，输出后退出
- REPL 支持多轮对话：输入 prompt → LLM 响应 → 可能的 tool 调用 → 继续等待输入

### Auth / API Keys

- DeepSeek：`DEEPSEEK_API_KEY` 环境变量
- OpenAI Codex：OAuth 流程（ChatGPT Plus/Pro 订阅）
- 镜像原版的 auth 抽象层

### 错误处理

- 使用 `thiserror` 定义错误类型
- 使用 `anyhow` 在边界处做错误传播
- 工具执行失败通过 `AgentToolResult` 中的 `isError` 表示，不 panic
- LLM provider 错误通过 `AssistantMessage { stopReason: "error", errorMessage }` 通信，不抛异常

## Testing Decisions

### 测试原则

- 所有测试 100% 本地运行，不依赖任何在线 API、LLM 端点、或本机已安装的 pi
- LLM provider 通过 mock provider 模拟
- 外部依赖（文件系统、进程）根据测试场景选择 mock 或真实执行

### 测试 Seam

单个高等级 seam：agent loop 入口处的 mock LLM provider。

```
MockProvider → agent loop → bash tool (real) → result → MockProvider
     ↑                                                 |
     └─────────────────────────────────────────────────┘
```

## Current Scope

### 已完成

- Agent 核心 loop（LLM ↔ tool 交互）
- 两个 LLM provider（OpenAI Codex、DeepSeek）
- Bash、Read、Write、Edit tools
- REPL 模式（Ctrl+C 取消、多轮对话）
- JSONL session 持久化
- Prompt Templates、Skills、System Prompt
- PrintFrontend（bare-terminal 事件消费层）
- Ratatui TUI（structured transcript、tool/thinking fold、scroll follow、multiline input、history、TestBackend snapshots、PTY smoke）
- AgentEvent 事件边界（含 RunId 隔离、ToolExecutionContext）
- CancellationToken 端到端传递
- TerminalGuard RAII 和 structured CommandResult
- Slash commands（/help, /exit, /quit, /model, /context, /session, /tree, /list-sessions）

### 当前非目标

- Session picker、model picker
- 文件树 UI
- 完整 Markdown 渲染、diff 编辑器
- 主题系统、鼠标交互、图像预览、插件 UI
- 自动补全、语法高亮
- 完整复刻 TypeScript TUI

## Out of Scope

- 扩展系统（动态加载 extension）
- 除 Codex 和 DeepSeek 外的其他 LLM providers
- Orchestrator（IPC 编排器）
- Session compaction / summarization

## Further Notes

本阶段产物仅包含源码、测试、文档、配置文件。不涉及 CI/CD、发布脚本、CHANGELOG。
