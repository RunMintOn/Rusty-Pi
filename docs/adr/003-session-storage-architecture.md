# ADR 003: Session Storage Architecture

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

Rusty-Pi needs durable JSONL sessions and isolated in-memory tests. Storage concerns, tree operations, context construction, and Agent execution should not be fused into one file-backed object.

Decision:

Use three layers:

```text
SessionStorage trait ← InMemorySessionStorage / JsonlSessionStorage
        ↑
      Session
        ↑
      Agent
```

`SessionStorage` provides persistence operations, leaf management, and path traversal. `Session` provides high-level entry, branching, metadata, and context operations. `Agent` interacts through the Session API. JSONL is the current durable storage representation; it is not a promise of PI Session-file compatibility.

Session tree entries use explicit serialized type tags. Entry timestamps and message timestamps remain distinct representations because they belong to different layers.

Consequences:

- Tests can use in-memory storage without filesystem side effects.
- Production can reopen JSONL sessions and resume through the CLI.
- Context transforms and compaction entries have a defined storage boundary.
- Branch APIs exist as infrastructure; interactive branch navigation and full lifecycle orchestration remain future work.
- Changes to the session wire format require an explicit migration decision.

Supersedes: None
Superseded by: None
