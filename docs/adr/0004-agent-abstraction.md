# 0004 — Providers vs Executors

**Status:** accepted

Two different capabilities are needed:

* **Provider** — text in, text out. Used for analysis (semantic commit
  extraction), compatibility checks (merge) and verification. Implementations:
  `anthropic`, `openai` (any OpenAI-compatible endpoint), `command` (any CLI
  that reads a prompt on stdin and prints a reply, e.g. `claude -p`,
  `codex exec -`), `scripted` (replay file), `none`.
* **Executor** — given a task brief and a directory, edit code. Implementations:
  * `builtin` — Gitent's own bounded tool loop driving any Provider;
  * `command` — any coding-agent CLI run inside the worktree with the brief on
    stdin (`GITENT_BRIEF`, `GITENT_WORKTREE` env vars). Presets:
    claude-code, codex, kiro, gemini, aider;
  * `host` — pause and let the calling agent (via MCP/CLI) do it.

**Why.** Mature coding agents already have excellent editing loops; Gitent
should orchestrate and validate them, not compete with them. The builtin
executor exists so Gitent works with a bare API key and is fully testable.
