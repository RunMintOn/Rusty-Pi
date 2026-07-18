# 003 — SSE Event Parsing Strategy

**Status:** Accepted (2026-07)

**Context:** OpenAI Codex Responses API 通过 SSE 流式返回事件。原版 TypeScript 参考实现（`openai-codex-responses.ts`）解析 SSE 的方式是读取 `data:` 行的 JSON 内容，从 JSON 的 `type` 字段获取事件类型。Rust 版需要实现同等的 SSE 事件分发。

**两种可行方案：**

- **A. 参考原版：** 只读 `data:` 行，从 parsed JSON 的 `type` 字段分发
- **B. 标准 SSE 解析：** 读取 SSE 的 `event:` 头和 `data:` 行，按 event 类型分发

**实际 Codex API 的 wire format**同时发送两者**：**

```
event: response.output_text.delta
data: {"delta": "Hello", "output_index": 0}
```

两种方案都能在生产环境正确工作。

**Decision:** 采用方案 B（`event:` 头分发）。

**理由：**

1. **更贴近 SSE 标准。** 标准 SSE 规范（W3C）定义 `event:` 作为事件类型字段，`data:` 作为事件载荷。方案 B 是更"正确"的 SSE 实现。
2. **payload 更简洁。** `response.output_text.delta` 的 `data:` 只有 `{"delta": "...", "output_index": N}`，不需要额外嵌套一层 `type`。方案 A 要求 API 在 `data:` 中重复 `type`，实际 API 确实包含，但读取 `event:` 头是更直接的语义映射。
3. **不引入 JSON 预解析依赖。** 方案 A 需要先将每段 `data:` 内容做 JSON 解析才能确定事件类型；方案 B 可以在确定事件类型后再决定是否、如何解析 data JSON。

**代价：**

- **测试 mock SSE 数据需要 `event:` 头。** 原版 TypeScript 测试使用纯 `data:` 行（内嵌 `type`）的 SSE mock 数据，Rust 测试不能直接复用同一格式。
- **如果 Codex API 未来去掉 `event:` 头**（极不可能），方案 B 会静默忽略所有事件，所有响应被 `_` 兜底分支捕获。方案 A 不受此影响。

**Rejected alternatives:**

| 方案 | 为什么放弃 |
|---|---|
| 使用现成 SSE parser crate（如 `eventsource-stream`） | 事件类型有限（~10 种），手动解析可控，避免额外依赖。 |

**相关上下文：**

- `do_codex_stream()` 函数中实现 SSE 事件解析
- `find_double_newline()` 辅助检索 `\n\n` 或 `\r\n\r\n` 分隔符
- 缓冲区处理：累积 byte chunks，按分隔符切分事件
