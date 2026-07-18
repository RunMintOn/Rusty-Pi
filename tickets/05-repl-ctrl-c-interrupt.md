# Add Ctrl+C interrupt support in REPL

**Blocked by:** None — can start immediately.

## What to build

The REPL currently lacks signal handling — pressing Ctrl+C kills the process. Implement graceful interrupt: during an LLM call or tool execution, Ctrl+C aborts the current round and returns to the prompt.

## Acceptance criteria

- [ ] Ctrl+C during agent execution gracefully aborts the current round
- [ ] The agent signals an aborted response (`StopReason::Aborted`)
- [ ] After abort, the REPL returns to the `>` prompt for a new input
- [ ] Ctrl+C at the idle prompt exits the REPL (or is a no-op — match the original behaviour)
- [ ] All existing tests still pass
