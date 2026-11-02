# Gitent

**A Git companion for agents.** Git records what the code *is*. Gitent also
records what each change is *for* and what it must *keep doing*, as checkable
requirements. It uses that to move changes (backports, fork syncs, merges of
parallel agent branches, reverts after refactors) between versions of a code
base that have drifted apart, and *proves* the result.

It is not a Git replacement: keep using Git day to day, and reach for Gitent
when a change has to land on code that has diverged. See
[docs/VISION.md](docs/VISION.md).

```
gitent init --agent claude-code      # or codex, kiro, gemini, aider, builtin, host
gitent commit -m "Add caching"       # records a structured intention next to the git commit
gitent log                           # history of intentions + provenance
gitent check                         # are all live requirements still satisfied?
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

1. records the intention and its **requirements** when you commit. Every test you
   add becomes a requirement, and Gitent proves each one fails before the change
   and passes after it ([docs/REQUIREMENTS.md](docs/REQUIREMENTS.md)). It also
   records the intention itself as a first-class, structured object
   (`intent`, `behavior`, affected symbols, and the exact Git snapshot that
   realised it);
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

Experimental MVP. Implemented end-to-end: `init`, `commit`, `check`, `diff`,
`log`, `show`, `annotate`, `cherry-pick`, `merge`, `rebase`, `revert`,
`status`, `continue`, `abort`, `mcp`, `serve`, `schema`.
[docs/SUCCESS_CRITERIA.md](docs/SUCCESS_CRITERIA.md) lists what is verified.

Eval ([evals/RESULTS.md](evals/RESULTS.md)) on **24 real Django backports**
(the human backport's tests are the hidden oracle) and 9 synthetic scenarios:

| | plain git | Claude Code alone | Gitent + Claude Code |
|---|---|---|---|
| real backports correct (19 are git conflicts) | 5/24 | 24/24 | 24/24 |
| false successes with a faulty agent (real / synthetic) | — | 9 / 8 | **0 / 0** |

Gitent does what Git cannot. With a capable agent it **ties** on correctness
(it does not beat it), at roughly 2× the wall time. Its distinct value is that
it never reports success on wrong or broken work, and that every result carries
provenance and requirements.

## Build

```bash
cargo build --release
cargo test
```

Requires `git` (≥ 2.30) on `PATH` (used for worktree management).
