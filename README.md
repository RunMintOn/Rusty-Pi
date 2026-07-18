# rusty-pi

Rust 重写 [earendil-works/pi](https://github.com/earendil-works/pi)（AI coding agent）。

## 入口

| 文档 | 用途 |
|---|---|
| [SPEC.md](SPEC.md) | 项目规格、用户故事、架构决策 |
| [tickets.md](tickets.md) | 工作分解与当前 frontier |
| [AGENTS.md](AGENTS.md) | Agent 开发规则 |
| [MAINTENANCE.md](MAINTENANCE.md) | 构建、测试、操作指南 |

## 快速开始

```bash
cd rusty-pi

# Mock（默认，无需 API key）
cargo run

# DeepSeek
DEEPSEEK_API_KEY=sk-xxx cargo run -- -p deepseek

# OpenAI Codex（需 ChatGPT Plus/Pro 访问令牌）
OPENAI_CODEX_TOKEN=xxx cargo run -- -p codex

# 单次 prompt
cargo run -- "用中文说你好"
```

## 选项

| 选项 | 说明 | 默认 |
|---|---|---|
| `-p, --provider` | mock / deepseek / codex | mock |
| `-m, --model` | 模型 ID | provider 默认 |
| `-P, --prompt-path` | Prompt 模板文件或目录（可重复） | — |
| `-S, --skill-path` | Skill 文件或目录（可重复） | — |
| `[PROMPT]` | 省略则进入 REPL | — |

## 测试

```bash
cargo test
cargo clippy
```

所有测试本地运行、不碰网络。当前 200 个测试。

需要 LLM provider 的 API key：见 [SPEC.md](SPEC.md)。

详细维护说明见 [MAINTENANCE.md](./MAINTENANCE.md)。完整规格见 [SPEC.md](./SPEC.md)。
