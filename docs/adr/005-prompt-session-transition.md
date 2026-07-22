# ADR 005: PromptSession as a Bounded Transition Layer

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

`PromptSession` began as a shared prompt-expansion seam, but the current implementation also owns canonical prompt state, system-prompt rebuilding, context files, resource reload, model and Agent access, and selected Command-facing business operations. Treating it as only a template helper is no longer accurate.

Decision:

Keep `PromptSession` as the current transition business layer. It may continue to provide the shared prompt/resource seam needed by the existing REPL, single-shot path, TUI, and Command system, but it must not grow without a deliberate boundary decision. Future orchestration will move incrementally to `SessionController`/`AgentSession` rather than requiring a broad refactor in this milestone.

Consequences:

- Existing code can keep using PromptSession without a disruptive rewrite.
- Documentation must describe its current responsibilities and its transitional status.
- New lifecycle concerns such as steering, follow-up, retry, compaction orchestration, branching, and hooks belong in the future controller direction.
- This ADR does not claim that SessionController exists or that those capabilities are Available.

Supersedes: None
Superseded by: ADR 006
