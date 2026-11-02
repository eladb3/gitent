# Requirements: specification

Design rationale: ADR 0008. This document is normative for the implementation.

## 1. The requirement object

Stored inside the semantic record (`requirements[]`), schema `gitent/semantic-change@2`:

```json
{
  "id": "req_3f9a1c2e",
  "statement": "Looking up the same user twice loads it from the database once",
  "category": "behavior",
  "kind": "test",
  "test": "test_cache_hit",
  "check": null,
  "origin": "auto",
  "proof": { "before": "fail", "after": "pass", "commit": "982e3a7c…" },
  "rebound_from": null
}
```

| field | values / meaning |
|---|---|
| `id` | `req_<8 hex>`. Stable for the life of the intention: kept by cherry-pick, rebase and merge; ended only by retire or revert |
| `statement` | one human-readable, testable sentence |
| `category` | `behavior` (must now be true) or `constraint` (must stay true, e.g. "public signature unchanged") |
| `kind` | `test`, `command` or `judged` (see §2) |
| `test` | for `test`: the test identifier as extracted by `diff::test_in_line` (e.g. `test_cache_hit`) |
| `check` | for `command`: `{ "run": "<shell>", "expect_exit": 0, "timeout_secs": 120 }` |
| `origin` | `auto` (an added test), `author` (CLI flag or host-supplied intent), `llm` (proposed by the analyzer) |
| `proof` | evidence from commit time: `before`/`after` ∈ `pass` · `fail` · `skipped` |
| `rebound_from` | for a ported requirement, the commit whose realisation it was re-bound from |

A record can also contain `retires: [{ "id": "req_…", "reason": "…" }]`.

## 2. Running a check

* **`test`** — `validation.test_selector` with `{test}` substituted, e.g.
  * `python3 -m pytest -q -k {test}`
  * `python3 -m unittest -k {test}`
  * `cargo test {test}`
  * `go test ./... -run '^{test}$'`
  * `npx jest -t "{test}"`

  `gitent init` detects a default. If no selector is configured, `test`
  requirements fall back to "the test exists **and** the full `test_command`
  passes" and get the warning `no_test_selector`.
* **`command`** — run `check.run` with `sh -c` in the snapshot. It passes if the
  exit code equals `expect_exit` within `timeout_secs`.
* **`judged`** — not executed. The verifier LLM gets the statement and the diff
  and returns `{satisfied, confidence, concerns}`. A judged result is never
  reported as a pass of an executable check.

## 3. Where requirements come from (`gitent commit`)

Applied in order and de-duplicated by `test` / statement:

1. **Auto.** Every test added by the diff becomes
   `kind: test, origin: auto`, with a statement derived from the test name
   (`test_cache_hit` → "cache hit") unless the LLM or author supplies a better one.
2. **Author.** CLI and API inputs:
   * `--require "<statement>"` followed optionally by `--check "<shell>"` (kind `command`) or `--test <name>` (kind `test`);
   * on its own, `--require` produces kind `judged`;
   * the same in the `requirements[]` array of an intent draft (MCP `gitent_commit`).
3. **LLM.** The analyzer proposes `requirements[]` (statement, category,
   optional `test`):
   * a proposal naming a `test` that is **not** added or modified in the diff is downgraded to `judged`, with a warning;
   * LLM-proposed `command` checks are **not accepted** (Gitent never runs commands an LLM made up). They are stored as `judged`.
4. **Migration.** When reading `@1` records:
   * `acceptance[]` → `judged/behavior`;
   * `constraints[]` → `judged/constraint`;
   * `tests[]` → `test/auto`.

## 4. Proof at commit time

For each executable requirement:

1. **after:** run the check in the new tree. If it fails, the commit is
   rejected (exit 2), unless `--allow-failing <id>` is given; in that case the
   requirement is stored with `proof.after = "fail"` and flagged.
2. **before:** create a temporary worktree at the parent, **overlay the
   commit's test files**, and run the check.
   * Test files are files containing an added test definition, plus files
     under a path segment named `test`, `tests` or `spec`, or matching
     `*_test.*` or `test_*.*`.
   * `fail` is expected. `pass` means the check does not discriminate this
     change: warning `non_discriminating:<id>`.

Policy is `requirements.prove = "warn" | "require" | "off"` (default `warn`).
`require` turns non-discriminating checks into errors. Proofs are skipped with
`--no-prove` and recorded as `skipped`.

## 5. The ledger

`ledger(C)` is the set of live requirements at commit `C`:

```
ledger(C) = ( ∪ over ancestors A of C, including C : introduced(A) )
            − retired(any ancestor) − reverted(any ancestor)
```

* `introduced(A)` = requirements in A's record whose `id` is new (re-bound
  copies from cherry-picks carry the same id, so they count once).
* Merges take the union of both parents' ledgers.
* Commits without a record contribute nothing (or `gitent annotate` them first).
* The result is cached in `.git/gitent/cache/ledger/<commit>` (an optimisation;
  it is always recomputable).

## 6. `gitent check`

```
gitent check [<rev>] [--intent <int_…>] [--judge] [--json]
```

* Without `<rev>` it checks the working directory, so it can run in CI and in
  pre-push hooks.
* With `<rev>` it checks a temporary worktree at that revision.
* It runs every executable requirement in the ledger. `--judge` also runs judged
  requirements through the verifier.
* It reports, for each requirement: id, statement, kind, result, the
  introducing intention (title + commit), and duration.
* Exit codes: 0 all pass · 2 an executable requirement failed · 1 error.
* An output line looks like this:
  `✗ req_3f9a1c2e "repeat lookup hits DB once" — from "Cache user lookups" (982e3a7)`

## 7. Requirements in operations

The validation step `requirements` replaces `source_tests_present`:

| operation | must pass | may disappear |
|---|---|---|
| cherry-pick | the source intention's requirements, re-bound on the target, plus `ledger(target tip)` | nothing |
| rebase (per step) | the step's source requirements plus the ledger of the running tip | nothing |
| merge | `ledger(ours)` ∪ `ledger(theirs)` | requirements retired on either side |
| revert | `ledger(tip)` minus the reverted intention's requirements | the reverted intention's requirements; if their checks still *pass* afterwards, warning `revert_incomplete:<id>` |

* **Re-binding.** `test` requirements are matched by test name in the result
  tree. The ported requirement keeps its `id` and records `rebound_from`.
  A missing test name means the check fails.
* **Brief.** The task brief lists every requirement to satisfy, with its kind
  and how Gitent will run it. This is the agent's definition of done.
* **Merge compatibility** (the pre-check in engine step 1) compares requirement
  *statements* from both sides instead of free-text intents. A pair judged
  contradictory with confidence ≥ threshold stops the merge unless `--force`.
* **Proof on the target.** For cherry-pick and rebase, Gitent also runs the
  ported executable checks against the target tip *without* the change (test
  files overlaid, as in §4). This shows the ported tests still discriminate.
  Results are recorded in `proof`.

## 8. Retiring requirements

```
gitent commit -m "Make TTL configurable" --retire req_3f9a1c2e "TTL is now configurable; covered by req_77ab…"
```

* A reason is mandatory. Retirements are shown in `gitent log` / `gitent show`
  as `retires req_… (from "<intent title>")`.
* `gitent commit --check-ledger` runs the full ledger before committing. A live
  requirement that fails and is not retired in the same commit rejects the
  commit.
* Without `--check-ledger`, regressions are caught by `gitent check` (CI).

## 9. Configuration

```toml
[validation]
test_command  = "python3 -m pytest -q"
test_selector = "python3 -m pytest -q -k {test}"

[requirements]
prove = "warn"            # warn | require | off
check_ledger_on_commit = false
judge_in_operations = true   # run judged requirements through the verifier during operations
```

## 10. API surface

* `gitent_commit` accepts the following, and the response includes `requirements` with proofs:
  * `requirements: [{statement, category?, kind?, test?, check?}]`
  * `retires: [{id, reason}]`
  * `check_ledger`, `prove`
* `gitent_check {rev?, intent?, judge?}` returns `{passed, results[]}`.
* `gitent_show` includes the requirements and the ledger delta (introduced and retired).
