# ADR 001: Extensions Compatibility Strategy

**Status:** Proposed (not yet decided)

## Context

pi 的 extension 系统是一套深度集成的 TS 接口（~1700 行类型定义），rusty-pi 无法直接加载 TS 模块。

## Options

- **A. 嵌入 JS 运行时** — 最兼容，但复杂度和体积代价大。
- **B. Rust 原生 API** — 最干净，但现有扩展全部重写。
- **C. RPC sidecar** — rusty-pi 纯 Rust，extension 由独立 JS 进程通过 JSON-RPC 调用。

## Decision (leaning)

**倾向 C。** Extension 支持是可选附件，不侵入核心代码。MVP 阶段跳过，后续通过 sidecar 进程增量添加。

## Consequences

- MVP 不含 extension 支持。
- 核心必须暴露稳定的内部接口（tool 执行、事件通知、session 操作）供未来 RPC 层调用。
- 如最终选择其他方案，核心代码无需重写，只需替换 RPC 适配层。
