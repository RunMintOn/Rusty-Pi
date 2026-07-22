# Current Architecture

This document describes the code that exists today. Future boundaries are labelled as direction and must not be read as implemented capabilities.

## Layer and data flow

```text
CLI arguments / REPL / TUI
          ↓
Shared input routing
          ↓
Built-in Command or PromptSession
          ↓
Agent
          ↓
Provider / Tool / Session storage
```

Input routing recognizes four paths:

```text
User input
   ↓
Shared input router
   ├── Built-in command
   ├── Skill / prompt template
   ├── Unknown slash
   └── Ordinary Agent prompt
```

An Agent prompt follows:

```text
PromptSession
   ↓
Agent
   ↓
Provider / Tools / Session storage
```

Output follows the single business event boundary:

```text
AgentEvent
   ├── PrintFrontend
   │      ├── single-shot
   │      └── thin REPL
   └── Ratatui TUI
```

`AgentEvent` is the only Agent-to-frontend business event path. Legacy text and tool callbacks have been removed. A run emits one terminal event: `RunFinished`, `RunAborted`, or `RunFailed`.

## Domain ownership

### Agent

Agent owns:

- the model/tool loop;
- `AgentEvent` production;
- run cancellation;
- tool execution;
- provider streaming.

Agent does not own:

- terminal rendering;
- command pickers;
- TUI state;
- user input history.

Agent and Provider do not write to the terminal. They emit data and events for frontend adapters.

### PromptSession

`PromptSession` is the current transition business layer. It owns:

- canonical prompt state;
- system-prompt rebuilding;
- Skill and prompt-template expansion;
- context files;
- resource reload;
- access to the current model and Agent;
- the parts of the business surface needed by the Command system.

Skill/template expansion becomes an Agent prompt. A built-in Command is not written as a user message into the Agent Session.

PromptSession is a transition layer and must not expand without a boundary:

> PromptSession is a transition layer; it must not continue growing without limit.

### Command system

The shared asynchronous Command system owns:

- built-in command parsing and execution;
- asynchronous session operations;
- frontend-neutral `CommandResult` values.

It does not own:

- direct terminal writes;
- direct `inquire` calls;
- creation of Agent user messages.

REPL and TUI use the same command registry and async command contract. The REPL supplies an `inquire` interaction adapter. The TUI supplies a non-picker interaction boundary and keeps terminal ownership in Ratatui.

### Frontends

`PrintFrontend` owns the single-shot and REPL adapters, stdout/stderr semantics, and an automation-friendly output sink. It consumes AgentEvent and never becomes a second Agent implementation.

Ratatui owns the primary interactive interface: input and terminal ownership, transcript state, command lifecycle, cancellation effects, and AgentEvent rendering. `TerminalGuard` restores terminal state on normal exit, errors, and panic unwinding.

The TUI currently provides multiline input, process-local history, scrolling/navigation, structured transcript blocks, async command lifecycle, and PTY smoke coverage. It does not provide native model/context pickers; no-argument `/model` and `/context` show usage. Thinking blocks can render an event if one exists, but production providers do not currently produce `ThinkingDelta`.

## Session storage boundary

The current Agent uses `PromptSession` and the high-level `Session` API. Session storage has JSONL and in-memory implementations. JSONL persistence, resume, listing, and tree inspection are current tested entry points. Branch APIs and thinking/compaction metadata are recorded as infrastructure in the capability matrix; interactive branch navigation and automatic compaction are future work.

## Future SessionController insertion point

The accepted future insertion point is:

```text
Frontends
    ↓
SessionController / AgentSession
    ↓
PromptSession / Agent / Session storage
```

The future controller should own:

- prompt lifecycle;
- steering;
- follow-up;
- retry;
- compaction orchestration;
- branching;
- model changes;
- enabled tools;
- context transformation;
- resource reload;
- hooks.

This controller is not present today. Its accepted architecture direction does not make any of those capabilities Available. See the [ADR directory](adr/) and [roadmap](roadmap.md).
