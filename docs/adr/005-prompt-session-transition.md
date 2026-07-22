# ADR 005: PromptSession as a Bounded Transition Layer

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

`PromptSession` began as a shared prompt-expansion seam, but the current implementation also owns canonical prompt state, system-prompt rebuilding, context files, resource reload, model and Agent access, and selected Command-facing business operations. Treating it as only a template helper is no longer accurate.

Decision:

Keep `PromptSession` as a bounded transition layer and move ownership and
orchestration into the task-owned `SessionController`. PromptSession remains
the canonical prompt-state/resource seam, but no frontend or CommandContext
may hold it. Future orchestration will move incrementally to the controller
rather than making PromptSession a second lifecycle owner.

ADR 006 records the related future controller direction. It complements this decision rather than superseding it.

Consequences:

- Existing prompt-state behavior remains in PromptSession without a disruptive
  wire-format or Agent-loop rewrite.
- The M1-A controller owns the long-lived PromptSession and exposes explicit
  business requests instead of internal-object access.
- Documentation must describe its current responsibilities and its transitional status.
- New lifecycle concerns such as steering, follow-up, retry, compaction orchestration, branching, and hooks belong in the future controller direction.
- M1-A makes only the ownership/lifecycle foundation Available. Steering,
  follow-up, retry, compaction orchestration, branching, and hooks remain
  Planned.

Supersedes: None
Superseded by: None
