# Stream bash tool output in real-time

**Blocked by:** 03 (truncation logic should be in place first, as truncation interacts with streaming boundaries).

## What to build

The bash tool currently captures all output after the process exits. For long-running commands the user sees nothing until completion. Stream stdout/stderr lines to the terminal as they arrive.

## Acceptance criteria

- [ ] Bash tool reads stdout/stderr line-by-line and emits text content incrementally
- [ ] Output lines appear on the terminal in real-time
- [ ] Truncation still applies (blocks on 03 being done)
- [ ] Timeout and abort still work correctly during streaming
- [ ] All existing tests still pass
