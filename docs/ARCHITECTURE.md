# Architecture

```
            ┌──────────── drivers ────────────┐
  human ──► │ CLI (clap)  --json              │
  agent ──► │ MCP server (stdio JSON-RPC)     │──► api::dispatch(tool, args) ──┐
  agent ──► │ HTTP API (POST /v1/tools/<t>)   │                                │
            └─────────────────────────────────┘                                ▼
 ┌─────────────────────────────── gitent core (Rust) ───────────────────────────────┐
 │ ops::engine      Operation state machine: plan → deterministic → agent → validate │
 │                  → commit-in-worktree → finalize (CAS branch update) / rollback   │
 │ ops::{commit, cherry_pick, merge, rebase, revert}  build steps for the engine    │
 │ analyze          diff → SemanticChange (LLM | host-supplied | heuristic), checked │
 │ validate         test command, test-preservation guards, empty-diff, verifier LLM │
 │ store            SemanticChange ⇄ refs/notes/gitent (JSON), op journal            │
 │ repo             git2: trees, diffs, in-memory cherry-pick/merge/revert, commits │
 │                  git CLI: worktree add/remove                                     │
 │ diff             unified diff text, symbol/test-name extraction (heuristics)      │
 │ llm::*           Provider trait: anthropic | openai | command | scripted | none   │
 │ agent::*         Executor trait: builtin (tool loop) | command | host            │
 │ tools            bounded workspace tools for the builtin agent (path-confined)   │
 └──────────────────────────────────────────────────────────────────────────────────┘
```

## Data model

### SemanticChange (one per realising Git commit)

Stored as pretty JSON in a Git note on the commit (`refs/notes/gitent`).
Full schema: `gitent schema`.

| field | meaning |
|-------|---------|
| `schema` | `"gitent/semantic-change@1"` |
| `intent_id` | Identity of the *intention*. Shared by every realisation (original commit, cherry-picks, rebased copies). `int_<12 hex>` |
| `kind` | `feature · fix · refactor · perf · test · docs · chore · revert · merge` |
| `title`, `intent`, `behavior` | What and why; the observable behaviour after the change |
| `changes[]` | Atomic sub-intentions `{summary, files, symbols}` |
| `affected.files[]`, `affected.symbols[]` | Deterministically extracted from the diff; LLM-claimed symbols carry `verified` |
| `constraints[]` | Things the change must preserve ("public signature of `get_user` unchanged") |
| `acceptance[]` | Behavioural statements that must hold (used by agents and the verifier) |
| `tests[]` | Test identifiers introduced/modified by the change (deterministic) — used as preservation guards |
| `dependencies[]` | Preconditions (other intents or required code) |
| `realization` | `{commit, parents, tree}` — the exact Git snapshot |
| `provenance` | `{operation, derived_from?, reverts?, composes[], executor, attempts, analysis, validation, created_at}` |
| `confidence` | 0..1 combined confidence |

### Operation (local journal, `.git/gitent/ops/<op_id>.json`)

An operation is a list of **steps** executed sequentially in one isolated
worktree (`.git/gitent/worktrees/<op_id>`). Step kinds:

* `apply {source_commit}` — cherry-pick / rebase step
* `revert {source_commit}`
* `merge {theirs}` — produces a 2-parent commit

States: `running → (awaiting_host) → succeeded | failed | aborted`.

## The operation engine

For every step:

1. **Deterministic attempt** — `git2` in-memory cherry-pick/revert/merge of the
   step against the current worktree tip. If conflict-free, the tree is
   checked out into the worktree and **validated**. If validation passes, the
   step is done with no LLM call (`provenance.executor = "git"`).
2. **Agent attempts** — otherwise the worktree is reset to the tip (or kept at
   the clean-but-failing result, so the agent sees the semantic breakage), a
   **task brief** is written (`brief.md`: intention record, original diff as
   reference, locations of affected symbols in the target, test command,
   rules, previous-attempt feedback) and handed to the configured executor.
3. **Validation** (see below). On failure, feedback is appended and the
   executor is invoked again, up to `validation.max_attempts`.
4. **Commit in worktree** — the worktree tree is written as a Git commit with
   the right parents and a semantic record (note) with provenance.

After all steps: **finalize** = compare-and-swap the target branch from the
recorded head to the new tip (fails if the branch moved), then a *safe*
checkout of the main working tree (which was verified clean at start). Any
failure leaves the branch untouched and the worktree on disk for inspection.

In `host` mode, step 2 does not run an executor: the operation pauses in
`awaiting_host` and returns `{op_id, worktree, brief}`. The host agent edits
the worktree and calls `gitent continue`, which runs step 3 onward.

## Validation

All of these are deterministic except the verifier:

| check | failure means |
|-------|---------------|
| `non_empty` | step produced no change |
| `tests` | configured `validation.test_command` exits non-zero (output captured as feedback) |
| `source_tests_present` | a test introduced by the source change (e.g. `def test_cache_hit`) is missing in the result |
| `existing_tests_preserved` | a test that existed at the target tip disappeared (agent deleted tests to go green) |
| `verifier` (optional) | LLM judged the diff does not satisfy the intention with confidence ≥ threshold |

Warnings (non-fatal, recorded): no test command configured; verifier
unavailable; agent touched files that contain none of the affected symbols.

## Storage layout

```
.gitent/config.toml               shared, committed config (test command, executor preset)
.git/gitent/config.toml           local overrides (e.g. which agent CLI is installed)
.git/gitent/ops/<op_id>.json      operation journal
.git/gitent/worktrees/<op_id>/    isolated worktree (git worktree)
.git/gitent/ops/<op_id>/brief.md  task brief handed to the executor
refs/notes/gitent                 semantic records (push/fetch to share)
```

## Extension points

* `llm::Provider` — add a model API.
* `agent::Executor` — add a coding agent integration.
* `diff::symbols` — swap heuristics for tree-sitter per language.
* `validate::Check` — add checks (type-checkers, linters, property tests).
* New operations are new step-builders over the same engine.
