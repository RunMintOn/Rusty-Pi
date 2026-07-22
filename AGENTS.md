# Rusty-Pi Agent Development Rules

## Source of truth

When sources disagree, use this order:

1. Source code and tests
2. `docs/capabilities.md`
3. `SPEC.md`
4. `docs/architecture.md`
5. Accepted ADRs in `docs/adr/`
6. `README.md`
7. Historical tickets

If README and source code conflict, do not guess. Inspect the relevant Rust source and tests first, then update the documentation or report the conflict.

`reference/earendil-works-pi/` is read-only research material. PI is a design reference, not a compatibility specification. Historical tickets are research and task records, not current implementation requirements.

## Product and architecture boundaries

- Rusty-Pi is an independent Rust coding agent.
- Do not mechanically translate PI. First read the current Rust code and the relevant PI source, then decide whether a capability should align, borrow, or be rejected.
- Keep Agent, Session, Command, and frontend ownership separate.
- Agent and Provider do not write to terminals; frontends consume `AgentEvent`.
- `AgentEvent` is the sole Agent-to-frontend business event path, with `RunFinished`, `RunAborted`, and `RunFailed` as the terminal event contract.
- Commands return frontend-neutral results, do not write to terminals, and do not create Agent user messages.
- Skills and prompt templates expand into Agent prompts.
- `PromptSession` is a bounded transition layer. Do not let it grow without an explicit SessionController decision.
- Do not add core built-in tools as a proxy for PI coverage; the fixed core is `bash`, `read`, `write`, and `edit`.
- A feature is not Available merely because a type or helper exists. Require production wiring and automated tests, and mark incomplete plumbing Infrastructure in `docs/capabilities.md`.
- Update the capability matrix when behavior changes.
- Record an accepted architecture decision in an ADR when a boundary or long-term direction changes.
- Do not use a historical ticket as the current specification.

## Development rules

- Work from the repository root for documentation and from `rusty-pi/` for Cargo commands.
- Read complex Rust modules and their relevant PI source completely before changing them.
- Use test-first development for core behavior.
- Keep normal tests offline and mock providers; do not call real LLM APIs from CI or ordinary tests.
- Prefer typed errors with `anyhow`/`thiserror`; avoid unnecessary `unwrap()` and `expect()` in production paths.
- Do not modify the reference tree.
- Do not clean up historical Clippy warnings unless the task explicitly asks for it.
- Do not implement roadmap features merely to make documentation true.

## Required validation

From `rusty-pi/`:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features
cargo test --locked
```

Do not write fixed test counts into documentation. Do not add `#[allow]` to hide warnings or `#[ignore]` to avoid running tests. New behavior needs production wiring and automated coverage.

## Known engineering pitfalls

### Process cleanup

Tools that spawn and wait for child processes must clean up the whole process group and reap children. The bash tool uses `std::process::Command`, an independent process group, `libc::killpg`, and blocking wait/reaping; do not reintroduce detached `tokio::process::Child` behavior.

### Cancellation

Cancellation must travel end-to-end through Agent, Provider, Command, and Tool boundaries. A tool that cannot observe the current cancellation token is not cancellable.

### TUI lifecycle

Keep `TerminalGuard` responsible for terminal restoration. The TUI command driver must await command settlement before accepting the next input. Keep `RunId` filtering so late events from an old run cannot alter a new transcript.

### Diagnostics

When a test hangs, inspect the process tree, thread wait channels, and zombie processes before changing async code:

```bash
pstree -p <test_pid>
cat /proc/<test_pid>/task/*/wchan | sort | uniq -c
ps -eo pid,ppid,stat,comm | grep Z
```

## Issue tracker and collaboration

Issues are local Markdown records. See `docs/agents/issue-tracker.md` and `docs/agents/triage-labels.md`. Preserve useful work from other agents: reread changed files and adapt instead of reverting collaborators' work.

Do not commit unless the task requests it. For this execution brief, all actual changes must be committed and `.gitignore` must remain untouched.
