# Capability Matrix

This is the authoritative list of what Rusty-Pi currently supports. Statuses describe product capability, not merely source-code presence:

> A type existing in code does not make a product capability Available.

## Status definitions

| Status | Meaning |
| --- | --- |
| Available | A formal entry point is usable and has automated tests. |
| Partial | A user can use the capability, but an important, explicit part is missing. |
| Infrastructure | Types or lower-level implementation exist without a complete user capability. |
| Planned | Not implemented; included in the accepted roadmap. |
| Not planned | Explicitly not intended for implementation or compatibility. |

## Runtime

| Capability | Status |
| --- | --- |
| Agent model/tool loop | Available |
| AgentEvent frontend path | Available |
| Run cancellation | Available |
| Provider transport cancellation | Available |
| Tool process cleanup | Available |
| Retry orchestration | Planned |
| Steering | Planned |
| Follow-up queue | Planned |
| Hook lifecycle | Planned |

`AgentEvent` is the sole business event path from Agent to frontend. `RunFinished`, `RunAborted`, and `RunFailed` form the terminal event contract.

## Providers

| Capability | Status |
| --- | --- |
| Mock Provider | Available |
| DeepSeek | Available |
| OpenAI Codex | Available |
| Generic OpenAI-compatible | Planned |
| Thinking types, metadata, events, and frontend scaffolding | Infrastructure |
| Provider thinking/reasoning transport | Planned |
| User-configurable thinking/reasoning | Planned |

The current provider set is not a claim of complete feature parity with any other product. Thinking data types and rendering exist, but production providers do not currently produce the complete reasoning stream.

## Tools

| Capability | Status |
| --- | --- |
| bash | Available |
| read | Available |
| write | Available |
| edit | Available |
| Per-session enabled tool selection | Planned |
| Additional core built-in tools | Not planned |

The four core tools are a product choice. Bash provides broad capability; the structured tools provide reliable file semantics. Core tool count is not measured against PI.

## Sessions

| Capability | Status |
| --- | --- |
| JSONL persistence | Available |
| Resume | Available |
| Session listing | Available |
| Tree inspection | Available |
| Branch data/API | Infrastructure |
| Interactive branch navigation | Planned |
| Thinking-level metadata | Infrastructure |
| Compaction entry/context transform | Infrastructure |
| Automatic compaction | Planned |
| Retry orchestration | Planned |

Compaction entry serialization, context transformation, summary-context reconstruction, and tests exist. Automatic thresholds, summary-model calls, `/compact`, and SessionController orchestration do not exist.

## Resources

| Capability | Status |
| --- | --- |
| Prompt templates | Available |
| Skills | Available |
| Context files | Available |
| Resource reload infrastructure | Infrastructure |
| Controller-owned resource reload orchestration | Planned |
| Unified Resource Loader product | Planned |

Skills and templates expand into Agent prompts; they are not command messages in the Agent Session.

## Frontends

| Capability | Status |
| --- | --- |
| Single-shot | Available |
| Thin REPL | Available |
| Ratatui TUI | Available |
| Async built-in commands | Available |
| TUI model/context native picker | Planned |
| JSON output mode | Planned |
| RPC/headless mode | Planned |

The no-argument `/model` and `/context` paths currently show usage in the TUI rather than opening an external picker. `inquire` belongs to the REPL adapter; the TUI does not hand terminal ownership to an external picker.

## Plugins and SDKs

| Capability | Status |
| --- | --- |
| PI extension compatibility | Not planned |
| Rusty-Pi plugin protocol | Planned |
| Rust SDK | Planned |
| TypeScript SDK | Planned |
| Optional PI adapter | Planned |

The optional adapter is lower priority and depends on a mature Rusty-Pi protocol. Direct PI extension loading is not promised.

## Automation and evaluation

| Capability | Status |
| --- | --- |
| Offline unit/integration tests | Available |
| Binary smoke tests | Available |
| PTY smoke tests | Available |
| Headless protocol tests | Planned |
| Live-provider evaluations | Planned |
| Agent-to-Agent coding evaluation | Planned |

Regular `cargo test` must remain offline. Live evaluations require explicit opt-in, may incur provider costs, and are not a substitute for deterministic CI. Model capability evaluation is not the same as deterministic CI.

## Thinking status

Existing Infrastructure:

- content/message types;
- thinking-level metadata;
- an `AgentEvent` variant (`ThinkingDelta`);
- frontend rendering scaffolding.

Not implemented:

- provider request option;
- DeepSeek/Codex reasoning parsing;
- production stream event wiring;
- complete persistence;
- user configuration entry.

The production chain is incomplete:

```text
Provider request setting
→ provider reasoning stream parsing
→ AgentEvent
→ Session persistence
→ frontend
```

Users cannot currently enable complete thinking/reasoning behavior. The types, metadata, events, and frontend scaffolding are `Infrastructure`; provider transport and user configuration are `Planned`.

## Compaction status

Compaction data/context infrastructure exists: compaction entries serialize to JSONL, context transforms apply, summaries can be reconstructed into context, and these behaviors have unit tests. Automatic compaction orchestration is not implemented: there are no automatic conditions or token threshold, no summary-model business flow, no `/compact` command, and no SessionController orchestration. The matrix therefore separates `Compaction entry/context transform` (`Infrastructure`) from `Automatic compaction` (`Planned`).
