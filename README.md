# Rusty-Pi

Rusty-Pi is an independent Rust coding agent. PI is the primary design reference, not a compatibility target: Rusty-Pi does not promise PI CLI, configuration, session-file, plugin, or behavior compatibility.

Each capability is decided independently: align, borrow, or reject. The project borrows useful business boundaries, session concepts, event contracts, and tested interaction behavior from PI while making independent Rust product decisions.

## Current status

The current binary has a tested Agent/tool loop, JSONL sessions, prompt templates, Skills, the four core tools, Mock/DeepSeek/OpenAI Codex providers, a thin REPL, single-shot execution, and a formal Ratatui TUI. Thinking content/message types, thinking-level metadata, AgentEvent support, and frontend rendering are infrastructure. Provider thinking/reasoning transport, user-configurable thinking/reasoning, and automatic compaction remain planned; see the [capability matrix](docs/capabilities.md).

## Quick start

```bash
cd rusty-pi
cargo build --locked
cargo run --locked --
```

Without a prompt, the binary enters the thin REPL. The default provider is `mock` unless configuration selects another provider, so the quick start is offline.

## Run modes

```bash
# Thin REPL
cargo run --locked --

# Ratatui full-screen frontend
cargo run --locked -- --tui

# One prompt, then exit
cargo run --locked -- "检查这个仓库"

# Resume a JSONL session by path or partial filename match
cargo run --locked -- --resume <path-or-prefix>

# List saved sessions and exit
cargo run --locked -- --list-sessions

# Add one or more context files to the system prompt
cargo run --locked -- --context AGENTS.md
```

Use `cargo run --locked -- --help` for the authoritative CLI help. Current resource flags are `--prompt-path`/`-P` and `--skill-path`/`-S`; both accept repeatable file or directory paths.

## Providers

Formal providers are:

- `mock` — deterministic local provider for development and tests;
- `deepseek` — configured with `DEEPSEEK_API_KEY`;
- `codex` — OpenAI Codex, configured with `OPENAI_CODEX_TOKEN` or stored OAuth credentials.

Examples:

```bash
cargo run --locked -- --provider deepseek "检查这个仓库"
DEEPSEEK_API_KEY=sk-xxx cargo run --locked -- --provider deepseek "检查这个仓库"
OPENAI_CODEX_TOKEN=xxx cargo run --locked -- --provider codex "检查这个仓库"
```

Provider quality is prioritized over provider count. A generic OpenAI-compatible provider is a future direction, not a current provider.

## Core tools

The fixed core built-in tool set is:

- `bash` — broad shell and process capability;
- `read` — structured file and image reading;
- `write` — reliable file creation and replacement;
- `edit` — structured text mutation.

Adding more core built-in tools is not a product goal.

## Sessions and resources

Sessions are persisted as JSONL under `~/.pi/agent/sessions/` by default. Set `RUSTY_PI_AGENT_DIR` to change the agent directory. Prompt templates, Skills, and context files are available. `PromptSession` contains reload infrastructure, but production reload orchestration remains planned for `SessionController`.

## Development

Run the complete offline validation from `rusty-pi/`:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features
cargo test --locked
```

Tests use mock providers and local seams; normal `cargo test` does not call a live provider or API. Binary and PTY smoke tests are part of the test suite.

## Documentation

- [Capability matrix](docs/capabilities.md) — the only authoritative list of current capabilities;
- [Architecture](docs/architecture.md) — current code boundaries and data flow;
- [SPEC.md](SPEC.md) — stable product direction and architecture principles;
- [Roadmap](docs/roadmap.md) — milestones, without date commitments;
- [ADR index](docs/adr/) — accepted architecture decisions;
- [TUI guide](rusty-pi/docs/tui.md) and [known issues](rusty-pi/docs/known-issues.md);
- [Maintenance](MAINTENANCE.md) — build, test, provider, session, and release operations;
- [Historical tickets](tickets.md) — research and task history, not the current implementation specification.
