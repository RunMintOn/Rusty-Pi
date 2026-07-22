# Rusty-Pi Product and Architecture Specification

## 1. Product statement

Rusty-Pi is an independent Rust coding agent distributed as a Rust binary. It prioritizes a reliable Agent/tool loop, explicit session persistence, a small core tool set, tested frontends, and offline-deterministic development.

PI is the primary design reference. Rusty-Pi learns from PI's business boundaries, session lifecycle, event model, frontend layering, validated interactions, and architectural trade-offs. Each capability is independently aligned, borrowed, or rejected.

## 2. PI reference policy

Reference code is evidence for terminology and design study, not a mechanical implementation specification. Rusty-Pi may adopt a principle without adopting PI's public API, implementation language, configuration, wire format, or user-visible behavior.

## 3. Non-compatibility policy

Rusty-Pi does not promise compatibility with:

- PI CLI flags or invocation conventions;
- PI configuration files;
- PI Session files or wire formats;
- PI plugins or extensions;
- every PI product behavior or feature;
- TypeScript implementation details.

Compatibility may be considered for a specific boundary only when it is separately accepted and documented.

## 4. Core tools

The product's fixed core built-in tools are `bash`, `read`, `write`, and `edit`. Bash supplies broad capability; the other three retain structured semantics and reliable file operations. Increasing the number of built-in tools is not a success criterion.

## 5. Provider strategy

The formal providers are Mock, DeepSeek, and OpenAI Codex. Mock is the offline test seam. DeepSeek uses `DEEPSEEK_API_KEY`; Codex uses `OPENAI_CODEX_TOKEN` or its stored credential flow. A generic OpenAI-compatible provider is a future direction. Quality, cancellation, and tested behavior take priority over provider count and unverified feature parity.

Thinking/reasoning types and display infrastructure exist, but a production provider does not currently complete the request-setting, reasoning-stream parsing, AgentEvent, persistence, and frontend chain. It is therefore not a completed product capability.

## 6. Session and business-layer direction

JSONL persistence, resume, listing, tree inspection, resource expansion, and context transformation are current capabilities where the capability matrix says Available. The current `PromptSession` is a transition business layer that owns canonical prompt state, system-prompt rebuilding, resource expansion, context files, and selected Agent/model access.

The long-term direction is a `SessionController`/`AgentSession` layer for prompt lifecycle, steering, follow-up, retry, compaction orchestration, branching, model changes, enabled tools, context transforms, resource reload, and hooks. This direction is accepted architecture, not an implementation claim.

## 7. Frontend strategy

- **Ratatui TUI:** the primary human interface, with terminal ownership, transcript state, input editing, command lifecycle, and AgentEvent rendering;
- **Thin REPL:** a compatibility, diagnostic, and automation adapter using the shared command and Agent business logic;
- **Single-shot:** a script and black-box test entry point using the PrintFrontend.

All three share Agent, session, and command business logic. Frontends do not independently implement those domains.

## 8. Plugin protocol direction

Rusty-Pi will eventually define its own language-independent, out-of-process plugin protocol. The planned sequence is protocol, Rust SDK, TypeScript SDK, and an optional low-priority PI adapter evaluated only after protocol maturity. Direct PI extension loading is not a product promise.

The protocol must define lifecycle, cancellation, isolation, and capability negotiation.

## 9. Platform priorities

Linux, macOS, and WSL are the priority platforms. Native Windows is not a current blocking condition or priority.

## 10. Quality and testing principles

Core behavior is developed test-first. Normal `cargo test --locked` runs offline with mock providers and local filesystem/process seams. Deterministic unit/integration tests, binary smoke tests, and PTY smoke tests are required for completed behavior. Live-provider evaluations, when added, must be explicit and are not deterministic CI.

## 11. Milestone roadmap

See [docs/roadmap.md](docs/roadmap.md) for M0-C through M5. The roadmap is a direction, not a release-date commitment. Success is closing the current business loop, not maximizing PI feature coverage.

## 12. Explicit non-goals

- mechanically translating PI;
- maximizing the number of built-in tools;
- directly loading PI plugins or extensions;
- making native Windows the current priority;
- putting more orchestration into frontends before SessionController exists;
- making default CI access a real provider;
- implementing every PI mode, flag, provider, or interaction.
