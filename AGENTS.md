# Development Rules

## Project Goal

用 Rust 完整重写 [earendil-works/pi](https://github.com/earendil-works/pi)（一个 AI coding agent），最终产物为 `rusty-pi`。

## Design Principle

**全权参考原实现。** 所有架构决策、接口设计、行为细节，一律以原版 TypeScript 代码为准，不自创、不臆想。原版代码位于 `reference/earendil-works-pi/`。

## Testing

- **测试先行。** 核心逻辑在实现之前先写测试。
- **全部本地运行，全部 mock。** 不使用任何在线 API、LLM 端点、或本机已安装的 pi。LLM provider 层使用 mock provider 返回预设响应，文件系统和进程操作根据测试场景酌情 mock。
- **目标是：Rust 版测试覆盖原版 TypeScript 测试的每一个行为点**，而非直接运行原版测试（原版为 vitest + TypeScript，无法在 Rust 中执行）。
- 非 e2e 测试应能通过 `cargo test` 一条命令全部运行通过。

## Code Quality

- 读透原版代码再动手改。对复杂模块，先完整阅读对应原版文件再做移植。
- 类型安全优先。善用 Rust 的类型系统，避免不必要的 `unwrap()` / `expect()`。
- 错误处理使用 `anyhow` / `thiserror`，不 panic。
- 遵循 Rust 社区惯例（clippy、rustfmt）。

## Working Directory

所有产物（代码、测试、文档、配置文件）均放置于本工作区 `pi-rust/` 下，不散落到系统路径或其他目录。

## Project Structure

```
pi-rust/
├── AGENTS.md               ← 本文件
├── reference/
│   └── earendil-works-pi/  ← 原版 pi 参考代码（只读，不修改）
├── rusty-pi/               ← Rust 项目（待创建）
│   └── ...
```

## Agent skills

### Issue tracker

Issues tracked as local markdown files in the repo root. See `docs/agents/issue-tracker.md`.

### Triage labels

Default labels configured (not actively used — solo project). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context repo. See `docs/agents/domain.md`.

## Commit

- 不要提交除非用户要求。
- 阶段性的成果确认后，由用户决定何时提交。
