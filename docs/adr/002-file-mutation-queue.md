# ADR 002: File Mutation Queue

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

Write and Edit can be invoked concurrently. Concurrent read-modify-write operations for the same path can lose updates or produce corrupted content, while unrelated paths should remain parallel.

Decision:

Use a shared per-path asynchronous mutex queue for file mutations. Write and Edit resolve a target path and serialize the complete mutation window for that path; different paths may proceed concurrently.

Consequences:

- Write and Edit share one mutation queue.
- The lock covers read-modify-write work, not only the final write.
- Callers do not manually manage queue lifetime.
- Cancellation is handled at operation checkpoints without releasing a mutation before it settles.
- The queue is an implementation boundary, not a new user-facing tool.

Supersedes: None
Superseded by: None
