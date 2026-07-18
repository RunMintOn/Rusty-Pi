# Implement SSE streaming for Codex provider

**Blocked by:** 01 (Codex regression fix must land first), 04 (refactored stream handling reduces merge friction).

## What to build

The Codex provider currently reads the full `/responses` response body at once. Change it to stream the response as SSE, emitting `StreamEvent::TextDelta`s token-by-token for real-time display.

## Acceptance criteria

- [ ] Codex provider uses streaming HTTP (SSE) instead of reading the full response body
- [ ] Text tokens are emitted as `StreamEvent::TextDelta` as they arrive
- [ ] Tool calls are emitted as `StreamEvent::ToolCall` when detected
- [ ] `Done` and `Error` events work correctly
- [ ] All existing tests still pass (mock-based, no network needed)
