# Known Issues and Current Limitations

This list contains only limitations still present in the current source and tests.

## TUI model/context picker is not native

`/model` and `/context` without an argument display usage in the Ratatui TUI. The TUI does not hand terminal ownership to `inquire`, and there is no native picker yet. Use an explicit command argument where supported or use the thin REPL adapter. Native TUI pickers are planned.

## Thinking/reasoning transport and configuration are incomplete

Thinking content/message types, thinking-level metadata, `AgentEvent::ThinkingDelta`, and frontend rendering exist as Infrastructure. Provider request options, DeepSeek/Codex reasoning parsing, production stream event wiring, complete persistence, and a user configuration entry are not implemented. Users cannot enable complete thinking/reasoning behavior; provider transport and user-configurable thinking/reasoning are Planned in [the capability matrix](../docs/capabilities.md), not supported reasoning.

## Automatic compaction is not orchestrated

Compaction entries, JSONL serialization, context transforms, and summary reconstruction are tested infrastructure. Automatic thresholds, summary-model calls, automatic business orchestration, and a `/compact` command are not present. Automatic compaction is `Planned` in [the capability matrix](../docs/capabilities.md).

## Future interfaces are not current interfaces

Steering, follow-up, retry, prompt queueing, automatic compaction, branch
navigation, hooks, JSON output, RPC/headless mode, native TUI session/tree
navigation, the independent plugin protocol, SDKs, and live-provider
evaluation are roadmap items. SessionController owns the M1-A lifecycle
foundation, but it deliberately does not provide those future operations. See
[the roadmap](../docs/roadmap.md) rather than treating these as current runtime
features.

If a newly observed problem cannot be confirmed from source and tests, record it as **Needs verification** instead of guessing.
