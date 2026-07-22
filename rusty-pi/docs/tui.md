# Ratatui TUI

Ratatui is Rusty-Pi's primary interactive frontend. Start it with:

```bash
cargo run --locked -- --tui
```

The TUI owns the terminal, renders a structured transcript, routes input through the shared asynchronous Command system, and consumes the unified `AgentEvent` stream. It is the formal primary interactive frontend.

## Input and history

The editor accepts multiline prompts. `Ctrl+J`, `Shift+Enter`, or `Alt+Enter` inserts a newline when the terminal reports the modifier. `Enter` submits a non-empty prompt when idle. While an Agent run is active, Enter is unavailable; the draft remains editable and is not queued concurrently.

Process-local prompt history records successful non-empty prompts and removes consecutive duplicates. History is not persisted to disk.

| Key | Input focus | Transcript focus |
| --- | --- | --- |
| `Tab` | Focus transcript | Focus input |
| `Up` / `Down` | Vertical editing; history on a single-line/empty draft | Select expandable block |
| `PageUp` / `PageDown` | Scroll transcript | Scroll transcript |
| `Home` / `End` | Input line start/end | Transcript beginning/follow latest |
| `Ctrl+A` / `Ctrl+E` | Line start/end | — |
| `Ctrl+U` / `Ctrl+K` | Delete to line start/end | — |
| `Ctrl+W` | Delete previous word | — |
| `Backspace` / `Delete` | Unicode-safe deletion | — |
| `Ctrl+C` while idle | Clear a non-empty draft | — |
| `Ctrl+C` while running | Cancel the Agent run | Cancel the Agent run |
| `/quit` or `/exit` | Exit | — |

## Transcript and navigation

The transcript renders User, Assistant, Thinking, Tool, Error, and System blocks. Assistant deltas merge into one block. Tool calls retain arguments and separate stdout/stderr streams, and tool states include running, success, failed, timed out, and aborted. Thinking blocks are collapsed summaries when `ThinkingDelta` events exist.

Scrolling follows the latest output by default. PageUp, Home, PageDown, End, selection movement, and unread-output tracking provide transcript navigation. Tool streams and the transcript have bounded memory with UTF-8-safe truncation.

## Commands and pickers

Commands use the same async registry and frontend-neutral results as the REPL. Command lifecycle is polled alongside redraw, resize, navigation, and cancellation; the driver waits for command settlement before the next prompt.

The TUI does not call an external terminal picker. `/model` and `/context` without arguments currently display usage; there is no native model or context picker yet. The `inquire` crate is confined to the REPL adapter.

## Cancellation and terminal ownership

`Ctrl+C` cancels the active Agent run or active Command through its cancellation boundary. It does not create a user-message transcript block. `TerminalGuard` restores terminal state on normal exit, command/Agent errors, and panic unwinding.

PTY smoke tests exercise the real TUI, multiline input, AgentEvent rendering, tool output and scrolling, command cancellation, Agent cancellation, exit, and terminal restoration. They use mock behavior and do not contact a live provider.

## Explicit limits

Thinking content/message types, thinking-level metadata, `AgentEvent::ThinkingDelta`, and TUI rendering are existing infrastructure. Provider request options, DeepSeek/Codex reasoning parsing, production stream event wiring, complete persistence, and a user configuration entry are not implemented, so TUI thinking rendering is not a user-enabled reasoning feature. Native session selectors, tree navigation, Markdown/diff product UX, and other pickers remain roadmap work; this document does not describe them as implemented.
