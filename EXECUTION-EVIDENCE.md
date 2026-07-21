# M0-B2 Execution Evidence

## A. Repository state

- Expected baseline: `158ecff16ac8953bd81e5be7685d277710b0b3b6`.
- Actual initial HEAD: `f3f1d7ad177b983a1ddca6a9b1932d125971b927` (`MAINTENANCE.md` documentation-only commit), accepted by the user before implementation.
- Initial branch: `master`.
- Initial status: ` M .gitignore` only.
- Implementation HEAD before this evidence document: `0103f0e`.
- Final delivery HEAD: see `git rev-parse HEAD`; this document is tracked separately from production changes.
- `.gitignore` was never staged, changed, restored, or committed.

## B. Async command boundary

`rusty-pi/src/coding_agent/command.rs` now provides:

- async-trait `Command::execute(&CommandInvocation, &mut CommandContext) -> Result<CommandOutcome>`;
- `CommandInvocation { name, raw_args }`, with `has_args()` and `trimmed_args()`;
- `CommandContext { session, interaction, cancellation }`;
- `CommandOutcome { result: Option<CommandResult>, control: CommandControl }`;
- `CommandControl::{Continue, Quit}`.

`/session`, `/tree`, and `/list-sessions` directly await Session/JSONL APIs. Tree previews use character-count truncation (`truncate_preview`) and never byte-slice UTF-8. Production command code has no `OnceLock`, `Runtime::new`, `block_on`, or `thread::scope` workaround.

`CommandResult::Quit` and `CommandResult::Noop` were removed. Quit is represented only by `CommandControl::Quit`.

## C. Input routing

`resolve_input()` in `command.rs` is shared by REPL and TUI:

| Input | Route | Agent called | Session message | TUI User block |
|---|---|---:|---:|---:|
| normal prompt | `AgentPrompt(original, expanded)` | yes | yes | yes |
| `/help` | `Command` | no | no | no |
| `/model codex` | `Command` | no | no | no |
| `/skill:known` | expanded `AgentPrompt` | yes | yes | yes, original text |
| `/template-known` | expanded `AgentPrompt` | yes | yes | yes, original text |
| `/unknown` | `UnknownSlash` | no | no | no |

The registry only recognizes built-ins. PromptSession now exposes exact `PromptExpansion` matching; Skill matching precedes template matching, and a matched expansion equal to the original text remains matched. Built-ins therefore take precedence over same-name templates. Unknown `/skill:xxx` is not sent to the provider.

## D. Interaction ownership

- REPL: `InquireCommandInteraction` in `coding_agent/repl.rs`; `Select` and `Text` run via `tokio::task::spawn_blocking`. User cancellation maps to `InteractionResult::Cancelled`; join and terminal errors remain `Err`.
- TUI: `UnavailableCommandInteraction`; it never reads stdin, writes stdout, imports the blocking picker, or changes terminal mode.
- Tests: `MockCommandInteraction`, `DelayedCommandInteraction`, and `FailingCommandInteraction`.

Static check:

```text
rg inquire rusty-pi/src/tui rusty-pi/src/main.rs rusty-pi/src/coding_agent/command.rs
(no matches)
```

The removed `Picker`, `RealPicker`, and `MockPicker` APIs have no remaining source/test references.

## E. TUI lifecycle

`AppState::submit()` records history and emits `Effect::SubmitInput` without creating a User block or starting an Agent run. Routing then emits either:

- `AgentPromptStarted { original, expanded }`: creates the original User block and starts the Agent with expanded text;
- `CommandStarted`: enters `CommandRunning`, with no User block;
- `InputRouteError`: creates one Error block for unknown slash input.

Activity states are `Idle`, `AgentRunning`, `AgentCancelling`, `CommandRunning`, and `CommandCancelling`. Ctrl+C emits the matching cancellation effect. Command Enter is rejected while running. `drive_command_with_ui()` in `main.rs` redraws, processes resize/navigation, polls the command future, and waits for cooperative cancellation without a detached task, extra runtime, or extra OS thread. Command completion renders System/Error blocks and returns to Idle; cancellation produces one `Command cancelled` block.

The existing Agent cancellation path remains covered by the prior and current PTY/unit tests.

## F. Command matrix

| Command | Async operation | REPL no-arg | TUI no-arg | Parameter mode |
|---|---|---|---|---|
| `/help` | registry metadata | metadata output | System block | N/A |
| `/exit`, `/quit` | none | quit control and save history | `Effect::Quit`, guard restores terminal | N/A |
| `/model` | provider model listing | Inquire select | usage message, no picker | exact model ID; already-current and invalid/list cases tested |
| `/context` | `tokio::fs::read_to_string` | Inquire input | usage message, no picker | raw path text retained; Unicode/spaces supported |
| `/session` | await metadata/count/model | message | System block | N/A |
| `/tree` | await entries | message | System block | UTF-8-safe preview |
| `/list-sessions` | async `read_dir`, JSONL open/entries | message/table | System block | corrupt JSONL skipped and counted |

## G. Session isolation

Command tests and TUI reducer tests verify that commands do not create Agent user transcript blocks or invoke the provider. `/context` changes only canonical system-prompt state. `/model` changes the active model without a provider request. `/help`, `/session`, `/tree`, and `/list-sessions` are read-only with respect to the Agent conversation. Command paths do not allocate an Agent RunId or emit AgentEvents.

## H. Skill/template evidence

`PromptSession::try_expand_prompt_command()` uses `skills::try_expand_skill_command()` followed by `prompt_templates::try_expand_prompt_template()`. Tests cover known Skill, known template, built-in/template precedence, unknown slash, unknown Skill, preserved argument/path text, and identity expansion.

## I. PTY evidence

`tests/pty_smoke.rs` now covers:

- `/model` without arguments: sees `Use: /model`, exits successfully within the existing five-second process timeout, and restores the terminal;
- `/context` without arguments: sees `Use: /context`, does not enter a hidden picker, exits successfully, and restores the terminal;
- `/context <temporary path>`: sees `Added`, exits successfully, and restores the terminal;
- existing prompt, tool, resize/navigation, Ctrl+C, and `/quit` cases.

PTY tests assert successful exit, join the reader thread, reap the child, verify the PID disappears, and check no zombie remains. PTY suite was run 30 consecutive times.

## J. PI references read

Reference files read:

- `packages/coding-agent/src/core/agent-session.ts`: `AgentSession.prompt`, `_tryExecuteExtensionCommand`, `_expandSkillCommand`, `sendUserMessage`, model management;
- `packages/coding-agent/src/modes/interactive/interactive-mode.ts`: interactive initialization, autocomplete composition, mode ownership, startup/run flow;
- `packages/coding-agent/src/modes/print-mode.ts`: `runPrintMode` and output/disposal flow;
- `packages/coding-agent/docs/usage.md`: slash command, session, prompt, model, and mode behavior;
- `packages/coding-agent/docs/extensions.md`: extension command handling, UI interaction ports, input routing, and `ctx.waitForIdle()`.

Adopted principles: command handlers are async and frontend-neutral; command checks precede prompt expansion; Skill/template expansion is distinct from command handling; frontend UI owns interaction. Rejected for this milestone: native TUI pickers, extension/plugin registry, Session Controller, RPC, and SDK compatibility.

## K. Test results

Final validation from `rusty-pi/`:

```text
cargo fmt --check: exit 0
cargo clippy --locked --all-targets --all-features: exit 0
cargo test --locked: exit 0
```

Final suite counts:

```text
discovered: 467
executed:   466
passed:     466
failed:     0
ignored:    1
```

Breakdown: 441 library tests, 7 binary tests, 10 binary smoke tests, 7 PTY tests, 1 passing doctest, 1 ignored doctest. No new Clippy warning category was introduced; existing historical warnings remain and were not hidden with new `allow` attributes.

Repeated validation:

```text
Full suite:             10 consecutive runs, all passed
Command/resolver:       30 consecutive runs, all passed (39 tests/run)
TUI lifecycle/state:     30 consecutive runs, all passed (24 tests/run)
PTY:                    30 consecutive runs, all passed (7 tests/run)
Race/hang observed:     none after the cancellation pre-check fix
Residual tasks/processes: none observed
```

## L. Commits

```text
3fa5708 refactor(commands): make command execution asynchronous
  Async command contract, invocation/context/outcome, built-in registry,
  async Session commands, exact expansion APIs, UTF-8 tree preview, picker removal.

6706aa5 refactor(commands): move interaction behind frontend adapters
  REPL Inquire adapter, async REPL routing, structured command rendering.

8327b95 fix(tui): drive slash commands without blocking terminal ownership
  Shared TUI resolver wiring, command lifecycle states/driver, deferred User block,
  command PTY smoke coverage and updated snapshots.

0103f0e fix(commands): honor cancellation before session listing
  Prevents a pre-cancelled list command from returning an empty result.
```

## M. Deviations

- The brief's accepted documentation-only HEAD divergence was used after explicit user confirmation.
- No native Ratatui model/file picker was implemented; TUI no-argument commands intentionally return usage messages.
- No full shell quoting parser, RPC, SDK, Session Controller, plugin registry, or single-shot built-in command behavior was added.
- The command driver remains wired to the existing crossterm event source in the binary; reducer/lifecycle behavior is independently testable without a real terminal.
- Existing Clippy warnings outside the changed command boundary were intentionally left untouched.

## N. Final Git state

Expected final status after committing this evidence document:

```text
 M .gitignore
```

The evidence document does not modify `.gitignore`.
