# 0014: Trust boundaries for the tool surfaces, CI and agents

**Status:** accepted. **Builds on:** 0004, 0005, 0009, 0012. **Prompted by:** the
external hands-on evaluation of 2026-11-07, security findings 1–7.

## Context

Gitent runs commands: test commands, test selectors, requirement checks, agent
CLIs and LLM CLIs. v0.1 assumed that every input selecting those commands was
trusted. That assumption is false in five places:

| input | who controls it | v0.1 behaviour |
|---|---|---|
| HTTP `gitent serve` | any local process; any web page, via CSRF or DNS rebinding | no auth; tool args could set `test_command`, `executor_command`, `test_selector` |
| MCP / HTTP `op_id` | the calling agent | joined into a path; `abort` could `remove_dir_all` anywhere |
| `.gitent/config.toml` at the PR head | the PR author | used by `verify-pr` in CI; `llm.base_url` beat the environment |
| `refs/notes/gitent` | anyone who can push a ref (branch protection does not cover notes) | `check.run` of command requirements executed on `check`/`verify` |
| test names substituted into `{test}` | anyone who writes a test (JS names are free text) | spliced unquoted into `sh -c` |

The agents Gitent drives also share the repository's `.git`, so an agent could
plant a hook that runs on the user's next git command.

## Decision

1. **HTTP requires a token.** `gitent serve` takes the token from
   `GITENT_TOKEN`; if it is unset, it generates a random one and prints it once on
   stderr. Every request needs `Authorization: Bearer <token>`. The `Host` header
   must name a loopback address. A request carrying a non-loopback `Origin` is
   refused. Failures return 401/403 with a JSON error.
2. **Tool surfaces cannot choose commands.** MCP and HTTP tool arguments no
   longer accept `test_command`, `test_selector`, `executor_command` or a
   provider command. Those settings come only from config and the CLI.
   `gitent_commit` over MCP stores agent-supplied `check.run` with origin
   `agent`, never `author`.
3. **`op_id` is validated.** It must match `^op_[A-Za-z0-9_-]{1,64}$`. Cleanup
   only deletes directories under the repository's worktree root, and only ones
   that `git worktree list` knows about.
4. **CI reads config from the base.** `verify-pr` loads `.gitent/config.toml`
   from the base commit, not the PR head. Command-kind requirements found in
   notes are not executed by `verify-pr` unless `--trust-notes` is passed;
   they are reported as skipped.
5. **Secrets go only where the user sent them.** Environment variables override
   `llm.base_url` from config. An API key is never sent to a plain `http://`
   URL unless the host is loopback.
6. **Substitutions are quoted.** `{test}`, `{file}` and `{module}` are
   shell-quoted wherever they are substituted.
7. **Timeouts kill the process group.** Commands run in their own process
   group, and a timeout kills the whole group, so jest workers and cargo test
   binaries cannot keep the pipes open.
8. **Agent containment.** Before an agent attempt, Gitent snapshots the shared
   `.git/hooks/` and `.git/config`. If the attempt changed them, Gitent restores
   them and fails the attempt. Worktrees live in a per-user, per-repository
   directory (`$TMPDIR/gitent-<uid>/<repo-hash>/`, mode 0700), not a shared
   `/tmp/gitent-worktrees`. The `gemini` and `kiro` presets print a warning that
   they run unsandboxed.
9. **The Action.**
   - The sticky comment is matched on marker *and* author (the token's bot user).
   - The PR title and tool output are escaped before they go into Markdown.
   - The release tarball is checked against its published `.sha256`.
   - `run.sh` maps exit codes explicitly: 0 = verified/neutral, 2 = rejected,
     3 = unverified, anything else = error.
   - A failed notes fetch is reported in the job log rather than ignored.

## Consequences

* MCP and HTTP clients that passed command overrides must move them into
  config. This is a breaking change to the tool input schemas, so the tool
  schemas' versions are bumped.
* `gitent serve` users must pass a token. The integration docs show how.
* Gitent still runs the repository's own test command; running a repository's
  tests runs its code. This ADR keeps *other* parties from choosing the commands.

## Known limits (after implementation)

* **Ctrl-C.** Commands run in their own process group, so Ctrl-C in a terminal
  no longer reaches them directly. Gitent kills the group when it times out or
  exits. Background processes that a command leaves running are killed 2 s
  after it exits.
* **Host mode.** Containment does not cover edits a *host* agent makes between
  `awaiting_host` and `gitent continue`. Covering them would need a snapshot
  stored in the op journal.
* **GitHub App tokens.** With an App installation token, `gh api user` fails
  and the sticky comment's author falls back to `github-actions[bot]`. The
  Action then posts a new comment on each run instead of updating its own.
