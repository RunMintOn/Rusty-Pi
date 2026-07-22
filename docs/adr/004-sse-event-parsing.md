# ADR 004: SSE Event Parsing

Status: Accepted
Date: 2026-07-22

Decision recorded from existing project direction.

Context:

The OpenAI Codex Responses API streams typed events over SSE. A parser must handle chunk boundaries, line endings, and event payloads without coupling stream ownership to a frontend.

Decision:

Parse SSE event headers and data payloads in the Codex provider. Accumulate byte chunks until an event boundary, use the `event:` value for dispatch, and parse the associated `data:` JSON for event content. Support LF and CRLF boundaries and retain request-scoped cancellation through the owned ProviderStream.

Consequences:

- Provider transport owns wire parsing and emits provider-neutral stream events.
- Tests can inject chunked, multiline, LF, and CRLF SSE data without a network call.
- Frontends do not know SSE details.
- The implementation is specific to the current Codex transport and does not imply general Provider feature parity.

Supersedes: None
Superseded by: None
