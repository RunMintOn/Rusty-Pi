# Fix Codex provider regression: restore tool_calls and stop_reason

**Blocked by:** None — can start immediately.

## What to build

Commit 2's streaming rewrite removed two things from the Codex provider:

1. The `tool_calls` field when building assistant messages for the request body — without it, the Codex API doesn't know about previously-executed tools, breaking multi-turn tool use.
2. The `status` field parsing and `stop_reason` derivation — `incomplete` → `StopReason::Length`, `failed` → `StopReason::Error`, `function_call` → `StopReason::ToolUse`.

Restore both, making Codex multi-turn tool calls work correctly again.

## Acceptance criteria

- [ ] Assistant messages in Codex requests include `tool_calls` when the message contains tool call content
- [ ] Response parsing extracts `status` and sets the correct `StopReason`
- [ ] `function_call` items in the response set `StopReason::ToolUse`
- [ ] All existing tests still pass
- [ ] Add a test that exercises multi-turn tool calls with Codex-style responses via MockProvider
