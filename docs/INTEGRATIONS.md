# Integrating Gitent with agents

There are two directions of integration, and they compose:

| Direction | Who does the semantic work | Typical use |
|-----------|---------------------------|-------------|
| **Gitent drives an agent** (`executor = command / builtin`) | the configured coding agent / LLM | humans or CI running `gitent cherry-pick …` |
| **An agent drives Gitent** (MCP / HTTP / `--json`, `executor = host`) | the calling agent itself | Claude Code / Codex / Kiro sessions using Gitent as their VCS |

## 1. Gitent drives a coding agent

`gitent init --agent <preset>` writes the matching config. You can also edit
`.gitent/config.toml` (shared) or `.git/gitent/config.toml` (local, wins).

```toml
[llm]                       # used for analysis / compatibility / verification
provider = "command"        # anthropic | openai | command | scripted | none
command  = "claude -p"      # prompt on stdin, reply on stdout

[executor]                  # used to change code
kind    = "command"         # builtin | command | host
command = "claude -p --permission-mode acceptEdits"
timeout_secs = 900

[validation]
test_command = "python3 -m unittest -q"
max_attempts = 2
```

A `command` executor runs **inside the isolated worktree** with:

* the full task brief on **stdin**,
* `{brief}` in the command replaced by the brief's path (for CLIs that take a
  prompt argument or message file),
* env vars `GITENT_BRIEF`, `GITENT_WORKTREE`, `GITENT_OP_ID`, `GITENT_ATTEMPT`.

The agent must only edit files. It must not commit — Gitent validates and
commits. Its stdout is kept in the operation journal.

### Presets

| preset | executor command | analysis provider | status |
|--------|------------------|-------------------|--------|
| `claude-code` | `claude -p --permission-mode acceptEdits` | `command: claude -p` | used for the evals in `evals/RESULTS.md` |
| `codex` | `codex exec --full-auto -` | `command: codex exec -` | per Codex CLI docs; verify with your version |
| `kiro` | `kiro-cli chat --no-interactive --trust-all-tools "Follow the task in {brief}"` | none | verify flags with your version |
| `gemini` | `gemini --yolo -p "Follow the task in {brief}"` | none | verify flags with your version |
| `aider` | `aider --yes-always --no-auto-commits --message-file {brief}` | none | verify flags with your version |
| `builtin` | Gitent's own tool loop | `anthropic` (needs `ANTHROPIC_API_KEY`) | |
| `openai` | Gitent's own tool loop | `openai` (`OPENAI_API_KEY`, `base_url` for Ollama/vLLM/…) | |
| `host` | pause and hand the brief to the caller | none | for MCP-driven use |

Any other agent: set `executor.kind = "command"` and the command line.
Environment overrides: `GITENT_EXECUTOR`, `GITENT_EXECUTOR_COMMAND`,
`GITENT_PROVIDER`, `GITENT_LLM_COMMAND`, `GITENT_MODEL`.

## 2. An agent drives Gitent

### MCP (stdio)

```bash
gitent mcp            # serves the repository in the current directory (or -C <path>)
```

* **Claude Code:** `claude mcp add gitent -- gitent mcp` (or copy
  `integrations/claude-code/.mcp.json`); the skill in
  `integrations/claude-code/skills/gitent/SKILL.md` teaches the workflow.
* **Codex:** add `integrations/codex/config.toml` to `~/.codex/config.toml`;
  append `integrations/codex/AGENTS.md` to your repo's `AGENTS.md`.
* **Kiro:** copy `integrations/kiro/mcp.json` to `.kiro/settings/mcp.json` and
  `integrations/kiro/steering/gitent.md` to `.kiro/steering/`.

Tools (all return JSON): `gitent_status`, `gitent_log`, `gitent_show`,
`gitent_diff`, `gitent_commit`, `gitent_cherry_pick`, `gitent_merge`,
`gitent_rebase`, `gitent_revert`, `gitent_continue`, `gitent_abort`,
`gitent_schema`.

Over MCP the default executor is **host**: when an operation needs code
changes, the tool returns `{"state":"awaiting_host","worktree":…,"brief":…}`.
The calling agent reads the brief, edits files **in that worktree**, then calls
`gitent_continue`. Gitent validates (tests, guards) and either commits or
returns the failures as feedback. Pass `"executor":"command"` to delegate to
the configured CLI agent instead.

For `gitent_commit`, a host agent should pass its own `intent` object (it
knows why it made the change); Gitent checks it against the diff.

### HTTP

```bash
export GITENT_TOKEN="$(openssl rand -hex 32)"   # or omit: serve generates one and prints it once on stderr
gitent serve --addr 127.0.0.1:7341
curl -s -H "Authorization: Bearer $GITENT_TOKEN" localhost:7341/v1/tools        # tools + JSON schemas
curl -s -H "Authorization: Bearer $GITENT_TOKEN" -XPOST localhost:7341/v1/tools/gitent_log -d '{"limit":5}'
```

Same tool names and arguments as MCP. Security (ADR 0014):

* Every request needs `Authorization: Bearer <token>`, where the token is
  `GITENT_TOKEN` or, if that is unset, a random token printed once on stderr
  at start-up. Missing or wrong token: `401` with a JSON error.
* The `Host` header must be a loopback name (`localhost`, `127.0.0.1`,
  `[::1]`), and a request with a non-loopback `Origin` is refused (`403`):
  web pages cannot reach the API through DNS rebinding or CSRF.
* Binds to localhost by default; do not expose it publicly.

### What tool arguments cannot do

MCP and HTTP tool arguments cannot choose commands: `test_command`,
`test_selector`, `executor_command` and `provider` are refused (they come only
from `.gitent/config.toml`, environment variables and CLI flags).
`gitent_commit` stores command checks an agent supplies with origin `agent`.
Operation ids must match `^op_[A-Za-z0-9_-]{1,64}$`. The tool input schemas
are versioned (`gitent/tools@2`; `_meta` of `tools/list`, `schema` of
`GET /v1/tools`, `x-gitent-tools` of `gitent schema`).

### Containment

Operation worktrees live in `$TMPDIR/gitent-<uid>/<repo-hash>/` (mode 0700).
Before each executor attempt Gitent snapshots the repository's `.git/hooks/`
and `.git/config`; an attempt that changes them is rolled back and failed
(`git_dir_integrity`). The `gemini` and `kiro` presets run their agents with
every tool auto-approved and no sandbox, and say so on stderr.

### CLI with `--json`

Every command accepts `--json` and prints exactly one JSON document on stdout
(errors: `{"error": "...", "code": "..."}` with non-zero exit). This is the
simplest integration for agents that only have a shell.
