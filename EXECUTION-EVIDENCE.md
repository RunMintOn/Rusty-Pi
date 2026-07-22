# M0-B2 Execution Evidence — correction

## Repository state

```text
Initial HEAD: b83f8dcd576df309853b12c02e96ff19e91c2007
Final code HEAD: ff5a2136eba31f7e77b65cebc85d0862e776f08b
Branch: master
Initial status: M .gitignore
```

`.gitignore` was not modified, staged, or committed by this execution. The two
implementation commits are:

```text
0a202c8 fix(commands): keep user errors inside the frontend
ff5a213 test(tui): exercise command polling and cancellation
```

## Context error matrix

| Scenario | Rust Err | CommandResult::Error | Frontend exits | State mutated |
| -------- | -------: | -------------------: | -------------: | ------------: |
| Missing file | no | yes | no | no |
| Directory path | no | yes | no | no |
| Invalid UTF-8 | no | yes | no | no |
| Cancelled read | no | no (`CommandOutcome::none`) | no | no |
| Interaction infrastructure failure | yes | no | caller-defined | no |

The `ContextCommand` maps `tokio::fs::read_to_string` errors—including
not-found, directory, and invalid UTF-8 errors—to `CommandResult::Error` while
preserving the canonical system prompt. Cancellation remains
`CommandOutcome::none()`. Interaction failures still propagate as `anyhow::Error`.

## REPL evidence

`context_error_keeps_repl_alive_for_following_prompt` drives the real chain:

```text
MockLineReader → shared resolve_input → CommandRegistry → ContextCommand → PrintFrontend<MemoryOutput>
```

The sequence `/context <missing>`, `ordinary prompt`, `/quit` renders one
context error, calls the mock Agent for the ordinary prompt, excludes `/context`
from the Agent session, leaves the system prompt unchanged, and exits through
`/quit`. Invalid UTF-8 has the same continuous-REPL coverage in
`context_invalid_utf8_keeps_repl_alive_for_following_prompt`.

## Driver evidence

`rusty-pi/src/tui/command_driver.rs` provides the injectable
`drive_command_core<C, E, R>` seam. The command future is pinned and awaited;
no command task is detached.

| Event | Command pending | State effect | Verified test |
| ----- | --------------: | ------------ | ------------- |
| No event tick | yes | redraw | `driver_redraws_on_no_event_ticks_until_command_finishes` |
| Synthetic Resize | yes | terminal size reduced | `driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw` |
| Synthetic PageUp | yes | browsing mode | `driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw` |
| Synthetic Enter | yes | no second submit/command | `driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw` |
| Synthetic Ctrl+C | yes | cancellation effect/token | `driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw` |
| Command settlement | until settled | one `CommandCancelled`, then Idle | `driver_waits_for_command_settlement_after_cancellation` |

The reducer test `command_error_is_one_error_block_idle_and_ready_for_next_input`
verifies that `CommandResult::Error` creates one Error block, returns to Idle,
creates no User block, and accepts the next input. Driver tests also verify no
Agent RunId or AgentEvent is created during command polling and that a normal
prompt can be submitted after cancellation.

## Lifecycle evidence

```text
Ctrl+C → token cancelled → command observed cancellation →
command settlement confirmed → driver returned → next input succeeded
```

The delayed test command observes cancellation only after a real pending period
and performs settlement work before returning. Redraw, resize, navigation,
Ctrl+C, settlement, and post-cancellation input were exercised without
`--test-threads=1`.

## PTY smoke

`real_tui_pty_ctrl_c_cancels_delayed_command_and_accepts_next_input` opts into
the debug-only `RUSTY_PI_TUI_TEST_DELAYED_COMMAND` registry seam, starts the
TUI on a real PTY, submits `/test-delayed`, sends navigation and real PTY
`0x03`, waits for `Command cancelled`, submits `/help`, then `/quit`. The PTY
helper waits for successful child exit, reaps the child, joins its reader, and
checks that the PID is gone and not a zombie.

No real provider or network endpoint is used.

## Routing boundaries

- `/` resolves to `UnknownSlash`, never an Agent prompt.
- Leading whitespace is normalized at the shared router, so REPL and TUI both
  expand `  /review src/` as the same template/skill route.
- No shell parser was introduced for this boundary.

## Validation

```text
cargo fmt --check                         passed
cargo clippy --locked --all-targets --all-features passed
cargo test --locked                       passed
```

The suite reported 453 library tests, 7 binary tests, 10 binary smoke tests,
8 PTY tests, 1 passing doctest, and 1 existing ignored doctest. No new
`#[ignore]` was added. The full suite passed 10 consecutive runs. Each of the
following passed 30 consecutive runs:

```text
context_error_keeps_repl_alive_for_following_prompt
context_invalid_utf8_keeps_repl_alive_for_following_prompt
driver_polls_pending_command_with_resize_navigation_ctrl_c_and_redraw
driver_waits_for_command_settlement_after_cancellation
driver_redraws_on_no_event_ticks_until_command_finishes
real_tui_pty_ctrl_c_cancels_delayed_command_and_accepts_next_input
```

Clippy's pre-existing warnings outside this change remain; no warning was
introduced by the new driver or command tests.

## Deviations

None from the requested scope. Native TUI pickers, Session Controller, RPC,
SDK, plugins, autocomplete, and visual redesign remain untouched. The delayed
PTY command exists only behind a debug-build test environment variable and is
not in the normal production registry.

## Delivery

```text
Archive: pi-rust-head-<final-head-7>.tar.gz
Archive SHA-256: reported with the final delivery
Bundle: pi-rust-head-<final-head-7>.bundle
```

Expected final working-tree status (the pre-existing user change only):

```text
 M .gitignore
```
