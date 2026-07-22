# M0-C Execution Evidence

## A. Repository state

```text
Initial HEAD: dd21b2e942f53eb440e5fcf63d6c40e4ff9ddc21
Validation HEAD: c47f4a4
Branch: master
Initial status: M .gitignore
Final status: M .gitignore
```

The pre-existing `.gitignore` change was not modified, restored, staged, or committed. The validation HEAD is the last implementation/documentation commit; this evidence file is a delivery record committed after validation and does not alter the implementation. The requested archive and bundle capture `c47f4a4`.

## B. Product positioning changes

Removed or replaced the old rewrite framing:

- Rusty-Pi is now described as an **independent Rust coding agent**.
- PI is described as the primary design reference, with capability-by-capability align/borrow/reject decisions.
- PI CLI, configuration, Session format, plugin, behavior, and TypeScript implementation compatibility are explicitly not promised.
- The fixed core tools are `bash`, `read`, `write`, and `edit`; tool-count parity is not a goal.
- Ratatui is documented as the primary frontend, with thin REPL and single-shot adapters.
- Thinking/reasoning and automatic compaction are documented as incomplete infrastructure/planned work rather than completed features.

## C. Documentation authority

| File | Audience | Authority | Main responsibility |
| --- | --- | --- | --- |
| `README.md` | First-time users | Orientation | Product identity, quick start, run modes, providers, tools, and index |
| `docs/capabilities.md` | Users and developers | **唯一 current capability list** | Status of what is currently supported |
| `SPEC.md` | Maintainers and designers | Stable direction | Product policy, boundaries, principles, non-goals, milestones |
| `docs/architecture.md` | Developers | Current structure | Real code ownership and data flow; future controller boundary |
| `docs/adr/` | Maintainers | Accepted decisions | Architecture decisions and consequences, not progress tracking |
| `MAINTENANCE.md` | Operators and contributors | Operational guidance | Build, tests, environment, sessions, terminal recovery, artifacts |
| `tickets/` | Historical research | Non-authoritative | Earlier research and task records |

The order is also recorded in `AGENTS.md`: source code/tests, capability matrix, SPEC, architecture, accepted ADRs, README, historical tickets.

## D. Capability matrix summary

| Area | Available | Partial | Infrastructure | Planned | Not planned |
| --- | --- | --- | --- | --- | --- |
| Runtime | Agent loop; AgentEvent path; run cancellation; provider transport cancellation; tool process cleanup | None | None | Retry orchestration; steering; follow-up queue; hook lifecycle | None |
| Providers | Mock; DeepSeek; OpenAI Codex | None | Thinking/reasoning transport | Generic OpenAI-compatible | None |
| Tools | bash; read; write; edit | None | None | Per-session enabled tool selection | Additional core built-in tools |
| Sessions | JSONL persistence; resume; session listing; tree inspection | None | Branch data/API; thinking-level metadata; compaction entry/context transform | Interactive branch navigation; automatic compaction; retry orchestration | None |
| Resources | Prompt templates; Skills; context files; resource reload | None | None | Unified Resource Loader product | None |
| Frontends | Single-shot; thin REPL; Ratatui TUI; async built-in commands | None | None | TUI model/context native picker; JSON output; RPC/headless | None |
| Plugins/SDKs | None | None | None | Rusty-Pi plugin protocol; Rust SDK; TypeScript SDK; optional PI adapter | PI extension compatibility |
| Automation/evaluation | Offline unit/integration tests; binary smoke; PTY smoke | None | None | Headless protocol tests; live-provider evaluations; Agent-to-Agent coding evaluation | None |

The full row-level matrix is `docs/capabilities.md`. It explicitly states that a type existing in source code does not make a product capability Available.

## E. Thinking and compaction

### Thinking

Source contains thinking content/message types, `ThinkingLevelChangeEntry` Session metadata, `AgentEvent::ThinkingDelta`, a PrintFrontend thinking renderer, and a TUI thinking block. Production providers currently have no complete chain for provider request setting, reasoning stream parsing, AgentEvent production, Session persistence, and frontend display. Users cannot enable complete thinking/reasoning behavior.

Final documentation marks `Thinking/reasoning transport` as `Infrastructure`.

### Compaction

Source contains `CompactionEntry`, JSONL serialization, context transforms, compaction-summary context reconstruction, and related unit tests. Source does not contain automatic conditions, a token threshold, model-generated summary orchestration, `/compact`, or SessionController orchestration.

Final documentation separates `Compaction entry/context transform` as `Infrastructure` from `Automatic compaction` as `Planned`.

## F. Frontend documentation

- **Single-shot:** `rusty-pi "prompt"`; PrintFrontend is the script and black-box test output path.
- **Thin REPL:** `rusty-pi` without a prompt; it shares the async Command system and uses `inquire` only in its interaction adapter.
- **TUI:** `rusty-pi --tui`; Ratatui owns terminal/input state, transcript, AgentEvent rendering, and command lifecycle.
- **Command:** built-in commands return frontend-neutral results, do not write terminal output, and do not create Agent user messages.
- **Picker boundary:** TUI does not call an external terminal picker. TUI `/model` and `/context` without arguments currently show usage; native TUI pickers are Planned.
- **Automation:** deterministic unit/integration tests, binary smoke tests, and PTY smoke tests all remain offline and use mock/local seams.

## G. ADR changes

| ADR | Status | Decision | Renamed/Supersedes |
| --- | --- | --- | --- |
| 001 Plugin protocol direction | Accepted | Own language-independent out-of-process protocol; Rust SDK first; TypeScript SDK later; no direct PI extension promise | Renamed from `001-extensions-strategy.md`; none superseded |
| 002 File mutation queue | Accepted | Shared per-path async serialization for Write/Edit mutation windows | Same number/title direction; none superseded |
| 003 Session storage architecture | Accepted | SessionStorage → Session → Agent, with JSONL and in-memory backends | Same number/title direction; no PI wire compatibility promise |
| 004 SSE event parsing | Accepted | Codex provider owns chunked SSE event/data parsing and cancellation | Renamed from duplicate `003-sse-event-parsing.md` |
| 005 PromptSession transition | Accepted | Keep PromptSession bounded as a transition layer | Renamed from `004-prompt-session-architecture.md`; superseded by direction in ADR 006 |
| 006 SessionController direction | Accepted | Future shared lifecycle/orchestration layer between frontends and PromptSession/Agent/storage | New; accepted direction, not implementation |

All ADRs now have unique three-digit prefixes and non-empty Status, Context, Decision, and Consequences fields.

## H. Historical documents

Added visible historical notices to:

- `tickets.md` — earlier MVP frontier;
- `tickets/feature-audit.md` — earlier PI feature audit;
- `tickets/spec-bare-terminal-architecture.md` — bare-terminal architecture proposal;
- `tickets/bare-terminal-capabilities.md` — bare-terminal capability research;
- `tickets/crate-reference-bare-terminal.md` — bare-terminal crate/API notes;
- `tickets/prompt-next-agent.md` — earlier agent handoff.

They remain because they preserve research and task history. Each points readers to `docs/capabilities.md`, `SPEC.md`, accepted ADRs, and source/tests instead of serving as current specification.

## I. Documentation contract tests

`rusty-pi/tests/documentation_contract.rs` adds six dependency-free integration tests:

- `readme_documents_current_entry_points_and_authoritative_docs` checks required README flags, links, and existing paths;
- `authoritative_documents_do_not_reintroduce_retired_positioning` scans the finite banned phrase list;
- `adr_numbers_and_required_sections_are_unique` checks three-digit uniqueness and required fields;
- `capability_matrix_defines_only_the_five_allowed_statuses` checks the status-definition table;
- `thinking_transport_is_explicitly_infrastructure` checks the exact capability row;
- `historical_plans_are_marked_and_point_to_current_facts` checks Historical notices and capability-source links.

No Markdown parser or new dependency was added. The test locates the repository root through `env!("CARGO_MANIFEST_DIR")` and its parent.

## J. PI references read

Read in full:

- `reference/earendil-works-pi/README.md`
- `reference/earendil-works-pi/packages/coding-agent/README.md`
- `reference/earendil-works-pi/packages/coding-agent/docs/usage.md`
- `reference/earendil-works-pi/packages/coding-agent/docs/extensions.md`
- `reference/earendil-works-pi/packages/coding-agent/src/core/agent-session.ts`

**Adopted terminology/principle:** separate the low-level Agent loop from a session lifecycle/business layer; describe session trees, compaction boundaries, input expansion, event lifecycle, and frontend adapters explicitly; treat extension lifecycle and cancellation as boundary concerns.

**Independent Rusty-Pi decision:** use Rust traits and owned cancellation/event streams, a fixed four-tool product set, Mock/DeepSeek/Codex as the formal provider set, Ratatui as the primary frontend, a thin REPL, and a future language-independent process plugin protocol.

**Compatibility explicitly rejected:** PI CLI/configuration/Session-file/plugin compatibility, direct TypeScript extension loading, and a requirement to reproduce every PI mode or user behavior.

## K. Test results

Baseline and final validation were run from `rusty-pi/` after the initial repository checks. The first shell attempt from the repository root found no `Cargo.toml`; this was logged as a papercut and immediately rerun from the crate directory.

```text
fmt exit code: 0
clippy exit code: 0
clippy errors: 0
total warning occurrences: 10 (pre-existing diagnostic occurrences)
unique warning categories: 5
new warnings: 0

discovered: 486
executed: 485
passed: 485
failed: 0
ignored: 1
filtered/skipped: 0

Full-suite repeated runs: 10/10 passed
```

The 10 warning occurrences are the existing `collapsible_if`, two `ptr_arg`, `unnecessary_map_or`, five `writeln_empty_string`, and `default_constructed_unit_structs` diagnostics. No warning cleanup or suppression was performed. No real network/API was used.

## L. Commits

| Hash | Title | Purpose | Files |
| --- | --- | --- | --- |
| `adfe11f` | `docs: establish product and capability truth` | Product positioning, authority, capability matrix, architecture, roadmap, README, maintenance, TUI, and known-issues truth | `README.md`, `SPEC.md`, `AGENTS.md`, `MAINTENANCE.md`, `docs/capabilities.md`, `docs/architecture.md`, `docs/roadmap.md`, `rusty-pi/docs/tui.md`, `rusty-pi/docs/known-issues.md` |
| `93c409f` | `docs: record architecture decisions and mark historical plans` | ADR renumbering/formatting and historical ticket notices | `docs/adr/`, `tickets.md`, six historical ticket files |
| `c47f4a4` | `test(docs): enforce documentation contracts` | Documentation contract tests and only necessary source-comment corrections | `rusty-pi/tests/documentation_contract.rs`, `rusty-pi/src/ai/providers/openai_codex.rs`, `rusty-pi/src/coding_agent/prompt_session.rs`, `rusty-pi/src/format/mod.rs` |

The evidence delivery commit is intentionally separate from the validated implementation commits above.

## M. Deviations

- No Agent, Provider, Tool, Session, TUI, or Command feature was implemented.
- Thinking transport, automatic compaction, SessionController, pickers, RPC, SDKs, plugins, new providers, new tools, and Session wire-format changes were not attempted.
- The repository root baseline shell invocation needed to be rerun inside `rusty-pi/`; this did not change the worktree and was logged with `papercuts`.
- Existing Clippy warnings were preserved as requested; no `#[allow]` or `#[ignore]` was added.
- Source comment corrections were limited to the stale Codex transport note, PromptSession transition ownership, and format-layer historical reference.
- No unresolved capability-status dispute or source-confirmation gap remained.

## N. Final Git state and artifacts

At validation and after the evidence delivery commit, the only worktree change is the pre-existing user edit:

```text
 M .gitignore
```

No changes are staged. The `.gitignore` diff remains the user's original planner-handoff and papercut-tool rule change.

Artifacts generated from validated HEAD `c47f4a4`:

```text
pi-rust-head-c47f4a4.tar.gz
pi-rust-c47f4a4.bundle
SHA256SUMS
```

SHA-256:

```text
30d47463a9da3713269688be5ae31a168a5341c3319cb29b67dc45b57e5d729f  pi-rust-head-c47f4a4.tar.gz
54ef11315eba2923742bd0689b40a723803bff79da479bb3bdc90c9593c6b9b7  pi-rust-c47f4a4.bundle
```
