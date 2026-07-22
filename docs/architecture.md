# Current Architecture

This document describes the code that exists today. Future boundaries are labelled as direction and must not be read as implemented capabilities.

## Layer and data flow

```text
CLI arguments / REPL / TUI
          ↓
Shared input routing
          ├── Built-in Command → SessionControllerHandle
          └── Prompt candidate → SessionControllerHandle
                                      ↓
                              SessionController task
                              ├── PromptSession
                              ├── Agent
                              └── Session storage
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

`AgentEvent` is the only Agent-to-frontend business event path. The
SessionController forwards it without reconstructing or altering events.
Legacy text and tool callbacks have been removed. A run emits one terminal
event: `RunFinished`, `RunAborted`, or `RunFailed`.

## Domain ownership

### Agent

Agent owns:

- the model/tool loop;
- `AgentEvent` production;
- run cancellation;
- tool execution;
- provider streaming.

The Agent is owned by the SessionController task. Frontends do not configure,
run, or cancel it directly.

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
- resource-reload infrastructure (a reload method and unit tests, without a CLI, Command, or TUI entry point);
- canonical prompt-state operations used by the controller;
- system-prompt rebuilding and resource expansion.

Skill/template expansion becomes an Agent prompt. A built-in Command is not written as a user message into the Agent Session.

PromptSession is a transition layer and must not expand without a boundary:

> PromptSession is a transition layer; it must not continue growing without limit.

### SessionController

The SessionController is a permanent, frontend-neutral Tokio task. It owns the
PromptSession, Agent, session storage, active run future, cancellation token,
and Agent event receiver. Frontends receive only a cloneable handle and one
event receiver. Requests use explicit oneshot replies; mutations received
while a run is active return `Busy` rather than queueing.

It currently owns prompt acceptance/rejection, resource expansion, model and
context mutations, read-only snapshots, tree access, cancellation, event
forwarding, and orderly shutdown. Steering, follow-up, retry, compaction,
branch navigation, hooks, and resource reload orchestration remain planned.

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

`PrintFrontend` owns the single-shot and REPL adapters, stdout/stderr semantics, and an automation-friendly output sink. It consumes AgentEvent forwarded by SessionController and never becomes a second Agent implementation.

Ratatui owns the primary interactive interface: input and terminal ownership, transcript state, command lifecycle, cancellation effects, and AgentEvent rendering. `TerminalGuard` restores terminal state on normal exit, errors, and panic unwinding.

The TUI currently provides multiline input, process-local history, scrolling/navigation, structured transcript blocks, async command lifecycle, and PTY smoke coverage. It does not provide native model/context pickers; no-argument `/model` and `/context` show usage. Thinking content/message types, thinking-level metadata, `AgentEvent::ThinkingDelta`, and frontend rendering are infrastructure. Provider request options, DeepSeek/Codex reasoning parsing, production stream event wiring, complete persistence, and user configuration are not implemented.

## Session storage boundary

The current Agent uses `PromptSession` and the high-level `Session` API. Session storage has JSONL and in-memory implementations. JSONL persistence, resume, listing, and tree inspection are current tested entry points. Branch APIs and thinking/compaction metadata are recorded as infrastructure in the capability matrix; interactive branch navigation, complete thinking/reasoning transport, and automatic compaction are future work.

## SessionController insertion point

The implemented insertion point is:

```text
Frontends
    ↓
SessionControllerHandle
    ↓
SessionController task
    ↓
PromptSession / Agent / Session storage
```

Future controller work should add:

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

The ownership/lifecycle foundation is Available. The listed future concerns
remain Planned until independently wired and tested. See the [ADR directory](adr/)
and [roadmap](roadmap.md).
