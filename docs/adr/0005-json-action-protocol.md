# 0005 — Builtin agent: JSON action protocol

**Status:** accepted

**Decision.** The builtin executor talks to providers with plain chat messages.
Each assistant turn must contain one JSON object
`{"tool": "<name>", "args": {...}}`; Gitent executes it and replies with
`TOOL_RESULT <name>:\n...`. `finish` ends the loop.

**Why not native tool calling.** Native tool calling differs per vendor and is
unavailable for CLI providers. A JSON protocol works identically with every
Provider, is trivial to record/replay for tests, and keeps the core
vendor-neutral. Native tool calling can be added later inside a provider
without changing the executor.

**Bounds.** Max steps (default 40), per-command timeout, output truncation,
and path confinement to the worktree (no absolute paths, no `..` escapes,
no writes into `.git`).
