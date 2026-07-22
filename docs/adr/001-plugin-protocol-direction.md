# ADR 001: Rusty-Pi Plugin Protocol Direction

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

Rusty-Pi is an independent Rust product and cannot treat TypeScript PI extensions as a native plugin ABI. A future plugin boundary must not couple the Agent core to a particular language runtime or give plugins implicit ownership of the host process.

Decision:

Rusty-Pi will define its own language-independent, out-of-process plugin protocol. The first SDK will be Rust, followed later by a TypeScript SDK. Direct PI extension compatibility is not promised. An optional PI adapter may be evaluated later, with lower priority, after the Rusty-Pi protocol is mature.

The protocol design must explicitly cover lifecycle, cancellation, process isolation, and capability negotiation. Protocol design precedes SDK implementation.

Consequences:

- The Agent core remains independent of a TypeScript runtime.
- Plugin failures and cancellation can be isolated at a process boundary.
- Rust and TypeScript clients can share a stable protocol rather than a language-specific ABI.
- Existing PI extensions will not load directly.
- Protocol and SDK work are Planned capabilities, not current runtime behavior.

Supersedes: None
Superseded by: None
