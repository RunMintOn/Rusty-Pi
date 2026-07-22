# Maintenance

This file contains operational instructions only. Product direction is in [`SPEC.md`](SPEC.md), current capabilities are in [`docs/capabilities.md`](docs/capabilities.md), and architecture is in [`docs/architecture.md`](docs/architecture.md).

## Rust and locked builds

Use the repository's Rust toolchain with the committed `Cargo.lock`. From `rusty-pi/`:

```bash
cargo build --locked
cargo fmt --check
cargo clippy --locked --all-targets --all-features
cargo test --locked
```

Normal tests are offline. Do not provide real credentials to ordinary tests.

## Providers and environment

- Mock: default when no configured provider is selected; requires no environment variable.
- DeepSeek: `DEEPSEEK_API_KEY` and `--provider deepseek`.
- OpenAI Codex: `OPENAI_CODEX_TOKEN` and `--provider codex`, or the stored Codex credential flow.
- `RUSTY_PI_AGENT_DIR`: optional agent directory override; default is `~/.pi/agent`.

Provider examples:

```bash
cargo run --locked -- --provider mock "local test"
DEEPSEEK_API_KEY=sk-xxx cargo run --locked -- --provider deepseek "prompt"
OPENAI_CODEX_TOKEN=xxx cargo run --locked -- --provider codex "prompt"
```

## Session files

The default JSONL session directory is `~/.pi/agent/sessions/`, or `$RUSTY_PI_AGENT_DIR/sessions/` when the override is set. `--resume <path-or-prefix>` opens a session, and `--list-sessions` lists readable JSONL sessions.

## Smoke tests

The ordinary suite includes binary smoke and PTY smoke tests. Run the focused PTY tests with:

```bash
cargo test --locked --test pty_smoke
```

PTY tests own a real pseudo-terminal and verify Ratatui input, command cancellation, Agent cancellation, output streaming, and terminal restoration without a live provider.

## Terminal recovery

If a process exits while owning the terminal, run `stty sane` in the affected terminal. The TUI's `TerminalGuard` restores raw mode, alternate-screen state, cursor state, and related terminal settings on normal exit, errors, and panic unwinding. If the terminal still looks incorrect, open a fresh shell or reset the terminal before rerunning a PTY test.

## Acceptance and archive artifacts

For an acceptance archive of the repository:

```bash
git archive --format=tar.gz --output=pi-rust-head-$(git rev-parse --short=7 HEAD).tar.gz HEAD
git bundle create pi-rust-$(git rev-parse --short=7 HEAD).bundle HEAD
sha256sum pi-rust-head-*.tar.gz pi-rust-*.bundle > SHA256SUMS
```

`EXECUTION-EVIDENCE.md` records validation and delivery evidence for an execution. Do not include build output or credentials in archives.

## Planner handoff ignore rule

The root `.gitignore` contains `/planner-handoff-*/` so planner handoff directories remain local and are not accidentally included in commits. That rule is maintained by the repository owner; do not change it as part of routine maintenance.

## Common issues

- A missing provider key is an environment/configuration error; use `mock` for offline checks.
- A session prefix matching multiple files must be made more specific.
- If parallel process tests hang, inspect for zombie children and process-group cleanup before changing test concurrency.
- If `/model` or `/context` is invoked without an argument in the TUI, it displays usage instead of opening a picker; native TUI pickers are future work.
