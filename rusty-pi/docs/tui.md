# Rusty-pi TUI interaction

The Ratatui frontend keeps a structured transcript and does not write agent or
command output directly to stdout. Start it with `rusty-pi --tui -p mock`.

## Focus and keys

The default focus is **Input**. `Tab` switches between the input editor and
transcript. `i` returns to input from transcript focus. `Esc` clears the current
transcript selection and returns to input. `q` is an ordinary input character;
it does not quit the application.

| Key | Input focus | Transcript focus |
| --- | --- | --- |
| `Enter` | Submit a non-empty prompt | Toggle selected Tool/Thinking block |
| `Ctrl+J` | Insert a newline | — |
| `Shift+Enter`, `Alt+Enter` | Insert a newline when the terminal reports the modifier | — |
| `Tab` | Focus transcript | Focus input |
| `Up` / `Down` | Edit vertically; on a single-line/empty draft, browse history | Move expandable-block selection |
| `PageUp` / `PageDown` | Scroll transcript | Scroll transcript |
| `Home` | Current input line start | Transcript beginning |
| `End` | Current input line end and follow latest | Follow latest |
| `Space` | — | Expand/collapse selected Tool/Thinking block |
| `Ctrl+A`, `Ctrl+E` | Line start/end | — |
| `Ctrl+U`, `Ctrl+K` | Delete to line start/end | — |
| `Ctrl+W` | Delete the previous word | — |
| `Backspace`, `Delete` | Delete Unicode characters safely | — |
| `Ctrl+C` | Clear a non-empty idle draft; no-op for an empty idle draft | — |
| `Ctrl+C` while running | Cancel the current run | Cancel the current run |
| `/quit` or `/exit` | Exit | — |

Enter is deliberately unavailable while an agent run is active. The next
prompt remains in the editor as a draft and is not queued or run concurrently.
A successful, non-empty submitted prompt is recorded in process-local history;
consecutive duplicates are removed. History is not written to disk.

## Transcript

Blocks are independent: `You`, `Assistant`, `Thinking`, `Tool`, `Error`, and
`System`. Assistant deltas append to one streaming block. Tool stdout and
stderr append to separate bounded fields and ToolFinished only updates the
existing Tool block. A Tool is collapsed by default and shows its name, state,
and exit code. Expanded tools show cached JSON arguments followed by separate
stdout and stderr sections. Thinking is collapsed by default and displays a
line-count summary.

The tool state labels are `running`, `success`, `failed`, `timed out`, and
`aborted`. Unknown tool IDs become an explicit `unknown` Tool block instead of
causing a panic.

## Scrolling and limits

The transcript follows new output at the bottom by default. `PageUp`, `Home`,
or an upward selection movement enters browsing mode. New content then leaves
the viewport in place and increments the unread count shown in the transcript
title. `End` (or scrolling back to offset zero) restores follow mode and clears
that count.

Each tool stdout and stderr stream retains at most 64 KiB. Overflow keeps a
UTF-8-safe head and tail around one `… output truncated …` marker. The frontend
also applies a soft 2,000-block transcript limit.

The input area grows from three rows to at most eight rows (and at most 30% of
the terminal height). It internally scrolls to keep the UTF-8 cursor visible.
The layout is tested down to 20x8 and accounts for double-width Unicode
characters.
