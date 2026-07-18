# Restore documentation comments stripped in commit 2

**Blocked by:** None — can start immediately.

## What to build

Commit 2 (streaming) stripped module-level and function-level doc comments from 5 files. Restore them so the codebase is readable again, especially given the project's principle of closely mirroring the original TypeScript — comments are the bridge between the Rust code and the reference.

## Acceptance criteria

- [ ] `src/ai/providers/deepseek.rs` — module-level docstring + public method docs restored
- [ ] `src/ai/providers/openai_codex.rs` — module-level docstring (including OAuth explanations) + public method docs restored
- [ ] `src/ai/mock.rs` — module-level docstring + MockStep variants docs + MockProvider behaviour contract restored
- [ ] `src/ai/providers/mod.rs` — field docs for `Provider`/`Model` restored
- [ ] `src/coding_agent/repl.rs` — module-level docstring + `run`, `run_single_shot`, `run_repl` docs restored
- [ ] No behavioural changes
- [ ] `cargo test` passes
- [ ] `cargo clippy` clean
