# Roadmap

This roadmap is directional, not a release-date commitment. Rusty-Pi does not use PI feature coverage as its success metric; it closes the existing business loop before adding breadth.

## M0-C — Documentation truth, ADRs, and capability matrix

Make product positioning, current runtime behavior, architecture boundaries, capability status, and historical planning documents agree.

## M1 — SessionController / AgentSession

- prompt lifecycle;
- steering;
- follow-up;
- retry;
- enabled tools;
- model change;
- controller-owned resource reload orchestration;
- context transform;
- compaction orchestration;
- branching;
- hooks.

## M2 — TUI productization

- native TUI pickers;
- session selector;
- tree navigator;
- Markdown;
- diff;
- tool UX;
- model/resource UX.

## M3 — Independent plugin protocol

- language-independent, out-of-process protocol;
- lifecycle;
- cancellation;
- isolation;
- capability negotiation;
- Rust SDK.

## M4 — Configuration, resources, models, and Session productization

- layered configuration;
- unified resource loader;
- provider/model registry;
- Session productization;
- migration and versioning.

## M5 — RPC, SDK, and Agent-to-Agent evaluation

- RPC/headless frontend;
- TypeScript SDK;
- live evaluation;
- Agent-to-Agent testing;
- optional PI adapter.

The order may change as evidence changes. No milestone is a promise to implement direct PI compatibility.
