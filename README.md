# Gitent

**Agent-native version control.** Git tracks textual changes. Gitent tracks
*semantic intentions* — what changed and why — and uses coding agents to carry
those intentions across diverged code when a textual patch no longer applies.

```
gitent init --agent claude-code      # or codex, kiro, gemini, aider, builtin, host
gitent commit -m "Add caching"       # records a structured intention next to the git commit
gitent log                           # history of intentions + provenance
gitent cherry-pick <commit>          # re-implements the intention on the current branch
gitent merge <branch>                # composes intentions from both sides
gitent rebase <onto>                 # replays intentions onto a new base
gitent revert <commit>               # removes an intention's behaviour, keeping later work
```

Gitent sits **on top of Git**. Every Gitent operation produces an ordinary Git
commit; the semantic record is stored in `refs/notes/gitent`, so a Gitent repo
is a Git repo and every Git tool keeps working.

## Why

`git cherry-pick` replays a *patch*. When the destination has renamed a
function, moved a file, or restructured the code, the patch conflicts (or,
worse, applies cleanly and is silently wrong). The *intention* — "cache user
lookups for 60 seconds" — is still perfectly applicable. Gitent:

1. records the intention as a first-class, structured object when you commit
   (`intent`, `behavior`, affected symbols, constraints, acceptance criteria,
   the tests that encode it, and the exact Git snapshot that realised it);
2. tries the deterministic Git operation first, and **keeps it only if it
   validates** (tests pass, nothing lost);
3. otherwise hands a bounded task brief to a coding agent in an isolated
   worktree, then validates the result (tests, test-preservation guards,
   optional LLM verifier) and retries with feedback;
4. only moves your branch when the result is validated, and records
   provenance linking the new commit to the original intention.

Your working tree is never touched until an operation succeeds, and it is never
overwritten if it has uncommitted changes.

## Built by agents, for agents

* **Any coding agent can be the engine.** Claude Code, Codex, Kiro, Gemini CLI,
  aider — or any command that edits files in a directory — plugs in as an
  *executor*. See [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md).
* **Any agent can be the driver.** Everything is available as JSON
  (`--json`), over **MCP** (`gitent mcp`) and over **HTTP** (`gitent serve`).
  In *host mode* the calling agent does the semantic work itself and Gitent
  does the deterministic orchestration and validation — no API key needed.
* **Developed docs-first.** [Vision](docs/VISION.md) →
  [Success criteria](docs/SUCCESS_CRITERIA.md) →
  [Architecture](docs/ARCHITECTURE.md) → [Decisions](docs/adr/) → code.
  Contributor agents start at [AGENTS.md](AGENTS.md).

## Status

Experimental MVP. Implemented end-to-end: `init`, `commit`, `diff`, `log`,
`show`, `annotate`, `cherry-pick`, `merge`, `rebase`, `revert`, `status`,
`continue`, `abort`, `mcp`, `serve`, `schema`.
See [docs/SUCCESS_CRITERIA.md](docs/SUCCESS_CRITERIA.md) for what is verified
and [evals/RESULTS.md](evals/RESULTS.md) for the comparison against plain Git
and a conventional coding agent.

## Build

```bash
cargo build --release
cargo test
```

Requires `git` (≥ 2.30) on `PATH` (used for worktree management).
