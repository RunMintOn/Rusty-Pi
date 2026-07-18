# Refactor stream event handling in agent loop

**Blocked by:** None — can start independently. Suggest ordering after 01 to avoid merge conflicts in Codex-related code.

## What to build

Two code smells in the agent's stream handling:

1. The agent loop in `engine.rs` manually processes `StreamEvent` variants (building `text_buf`, `content_buf`) while the same logic exists in `MessageAccumulator::push()`. Make the engine use `MessageAccumulator` instead of duplicating event handling.
2. Tool calls are passed around as `Vec<(String, String, serde_json::Value)>` instead of the already-defined `AgentToolCall` struct.

## Acceptance criteria

- [ ] Agent loop uses `MessageAccumulator` to accumulate stream events into an `AssistantMessage`
- [ ] Content and tool calls are extracted from the built `AssistantMessage`, not from ad-hoc buffers
- [ ] `AgentToolCall` is used instead of the tuple throughout the agent loop
- [ ] No behavioural change — existing tests cover the same scenarios
- [ ] `cargo test` passes, `cargo clippy` clean
