# Add bash output truncation

**Blocked by:** None — can start immediately.

## What to build

The bash tool currently returns the full stdout/stderr to the LLM with no size limit. The spec and tickets require truncation behaviour matching the original pi: when output exceeds a threshold, truncate and append a message indicating truncation occurred.

## Acceptance criteria

- [ ] Tool output is truncated when it exceeds a configurable size limit
- [ ] Truncated output appends a clear message (e.g. "(output truncated at N characters)")
- [ ] Default limit matches the original pi's behaviour
- [ ] Tests: output under limit is returned in full; output over limit is truncated
- [ ] All existing tests still pass
