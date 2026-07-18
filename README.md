# rusty-pi

Rust 重写 [pi](https://github.com/earendil-works/pi)（AI coding agent）。

## 入口

| 文档 | 用途 |
|---|---|
| [SPEC.md](SPEC.md) | 项目规格、用户故事、架构决策 |
| [tickets.md](tickets.md) | 工作分解与当前 frontier |
| [AGENTS.md](AGENTS.md) | Agent 开发规则 |
| [MAINTENANCE.md](MAINTENANCE.md) | 构建、测试、操作指南 |

## 快速开始

```bash
cd rusty-pi && cargo build
cd rusty-pi && cargo run            # REPL 模式
cd rusty-pi && cargo run "prompt"   # 单次 prompt
cd rusty-pi && cargo test           # 全部测试
```

需要 LLM provider 的 API key：见 [SPEC.md](SPEC.md)。
