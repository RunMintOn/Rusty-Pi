# Implement OAuth authentication for Codex provider

**Blocked by:** None (technically independent). Low priority — not a blocker for basic Codex usage.

## What to build

The Codex provider currently reads a manually-obtained access token from `OPENAI_CODEX_TOKEN`. Implement the OAuth flow matching the original TypeScript implementation, so the token can be obtained programmatically.

## Acceptance criteria

- [ ] OAuth flow matches the original `openai-codex-responses.ts` implementation
- [ ] Credentials are stored and refreshed as needed
- [ ] The existing env-var fallback still works as a development convenience
- [ ] All existing tests still pass
