# Tickets: rusty-pi MVP

从零搭建 Rust 版 pi 的核心功能：agent loop + 两个 LLM provider + bash tool + REPL。完整 spec 见 `SPEC.md`。

Work the **frontier**：每个 ticket 的依赖（Blocked by）全部完成后即可开始。按序推进。

## Ticket 1 — 项目脚手架 + 核心类型

**What to build:** rusty-pi 的 Cargo 项目骨架、模块结构、核心类型定义和 trait 体系。不包含任何运行时行为——只让结构和类型就位。

**Blocked by:** None — can start immediately.

- [ ] `cargo init rusty-pi`，配好 workspace 预留
- [ ] 模块结构镜像原版 5 个 package：`agent/`、`ai/`、`coding_agent/`、`tui/`（占位）、`orchestrator/`（占位）
- [ ] 依赖选型落地（tokio、serde、clap、reqwest 等）
- [ ] 核心消息类型：`UserMessage`、`AssistantMessage`、`ToolResultMessage`、Content blocks（Text、ToolCall、Image）
- [ ] `Tool` trait（name、description、parameters schema）
- [ ] `AgentTool` trait（execute、prepareArguments、executionMode）
- [ ] `AgentToolResult` 结构体
- [ ] rustfmt、clippy 配置
- [ ] `cargo build` 通过，`cargo test` 跑通

## Ticket 2 — MockProvider + Agent Loop + REPL

**What to build:** 一个 mock LLM provider 接收 prompt 并返回预设响应，围绕它搭建 agent loop 骨架（单回合 prompt → response），外加一个简陋的 REPL 入口。这是第一个"能跑起来看到东西"的里程碑。

**Blocked by:** Ticket 1

- [ ] `MockProvider`：实现 `Provider` trait，根据配置返回预设 text 或 tool call 响应
- [ ] Agent loop 单回合：接收 prompt → 调用 provider → 返回响应
- [ ] Agent harness 骨架（事件通知、tool call 调度）
- [ ] 简陋 REPL：`cargo run` → 输入 prompt → 看到 mock 回复 → 退出
- [ ] `cargo run "prompt"` 单次模式
- [ ] 测试：MockProvider + agent loop 单回合，验证 text response 路径

## Ticket 3 — Bash Tool

**What to build:** bash tool 实现——spawn 子进程执行命令、捕获输出、处理 timeout 和 abort。Wire 进 agent loop，使 mock provider 触发的 tool call 能被 bash 执行并返回结果。

**Blocked by:** Ticket 2

- [ ] `BashParams` 类型（command: String, timeout: Option\<u64\>）
- [ ] Bash tool execute：spawn 进程 → stream 输出 → 捕获 exit code
- [ ] timeout 支持（超时 kill 进程树）
- [ ] abort signal 支持
- [ ] 输出截断（参考原版的 truncation 行为）
- [ ] REPL 中展示 bash 输出
- [ ] 测试：agent loop 中 mock provider 触发 tool call → bash 执行 → 结果回 provider
- [ ] 测试：bash tool 纯单元测试（正常执行、错误退出、timeout）

## Ticket 4 — DeepSeek Provider

**What to build:** 真实的 DeepSeek LLM provider（OpenAI 兼容 API），通过 `DEEPSEEK_API_KEY` 环境变量认证，实现 streaming 调用。

**Blocked by:** Ticket 3

- [ ] DeepSeek provider 定义（id、name、base_url、model 列表）
- [ ] `DEEPSEEK_API_KEY` 环境变量认证
- [ ] HTTP streaming chat completions 调用（reqwest SSE）
- [ ] 响应解析为 `AssistantMessage`（text + tool calls）
- [ ] 模型列表（deepseek-v4-flash、deepseek-v4-pro）
- [ ] REPL 中可切换 provider（`--provider deepseek`）
- [ ] 测试：MockProvider 替代 DeepSeek 做同样的 streaming 行为验证

## Ticket 5 — OpenAI Codex Provider

**What to build:** 真实的 OpenAI Codex provider（ChatGPT Plus/Pro 订阅），实现 OAuth 登录 + HTTP SSE streaming + WebSocket 支持。

**Blocked by:** Ticket 3

- [ ] Codex OAuth 认证流程（参考原版 `openai-codex-responses.ts`）
- [ ] HTTP SSE streaming 实现
- [ ] WebSocket 传输实现（可选，原版作为备选传输）
- [ ] 响应解析为 `AssistantMessage`
- [ ] 模型列表（gpt-5.x-codex 等）
- [ ] REPL 中可切换 provider（`--provider codex`）
- [ ] 测试：MockProvider 替代 Codex 做同样的 streaming 行为验证

## Ticket 6 — 多轮 REPL + Session 模型

**What to build:** 从单轮到多轮：REPL 保持对话状态，prompt 历史累积发送给 LLM，agent loop 能处理 LLM → tool → LLM 的多回合交互。引入内存 session 树来跟踪对话状态。

**Blocked by:** Ticket 4, Ticket 5

- [ ] 多轮 REPL：prompt → LLM → tool → result → LLM → 继续等待输入
- [ ] 内存 session 树结构（id/parentId 链）
- [ ] 对话历史在 REPL 中的累积和传递
- [ ] 消息内容在终端中的清晰展示（text + tool calls + tool results + errors）
- [ ] 支持 Ctrl+C 中断当前回合
- [ ] 测试：多回合 agent loop（text → tool call → tool result → text）
