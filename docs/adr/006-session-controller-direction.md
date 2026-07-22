# ADR 006: SessionController / AgentSession Direction

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

Prompt lifecycle and session orchestration are currently spread across transition-layer and runtime seams. A durable business boundary is needed before adding steering, follow-up, retries, automatic compaction, branching UX, and lifecycle hooks to individual frontends or to PromptSession without limit.

Decision:

Introduce a task-owned `SessionController` layer between frontends and the
current PromptSession, Agent, and Session storage layers:

```text
Frontends
    ↓
SessionControllerHandle
    ↓
SessionController task
    ↓
PromptSession / Agent / Session storage
```

The controller currently owns prompt lifecycle, resource expansion, model and
context mutations, snapshots, cancellation, AgentEvent forwarding, and
shutdown. Its intended future responsibilities include steering, follow-up,
retry, compaction orchestration, branching, enabled tools, context
transformation, resource reload, and hooks. Frontends remain adapters and must
not independently implement these concerns.

Consequences:

- Lifecycle behavior will have one business owner shared by REPL, TUI, single-shot, and future headless modes.
- PromptSession can remain a bounded transition seam while responsibilities move gradually.
- The ownership/lifecycle foundation is production-wired and tested; it is
  Available in the capability matrix.
- The direction enables future native pickers and protocol frontends without
  duplicating Agent/Session behavior.
- Accepted architecture direction does not make the future orchestration
  responsibilities Available; they remain Planned until independently wired
  and tested.

Supersedes: None
Superseded by: None
