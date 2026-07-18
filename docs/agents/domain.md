# Domain Docs

Single-context repo.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root
- **`docs/adr/`** for past architectural decisions

If any of these files don't exist, proceed silently. The `/domain-modeling` skill creates them lazily when terms or decisions get resolved.

## File structure

```
pi-rust/
├── CONTEXT.md
├── docs/adr/
│   └── ...
├── reference/earendil-works-pi/  ← original pi reference (read-only)
├── SPEC.md
├── tickets.md
└── src/
```

## Use the glossary's vocabulary

When naming domain concepts, use terms as defined in `CONTEXT.md`. Don't drift to synonyms.

## Flag ADR conflicts

If output contradicts an existing ADR, surface it explicitly.
