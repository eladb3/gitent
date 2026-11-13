# 0018: `gitent bench`, benchmark coding agents on your own history

**Status:** accepted. **Builds on:** 0004 (executors), 0016 (scrubbed environments), 0017
(the per-test proof).

## Context

Public agent benchmarks measure public repositories. A team choosing or
configuring a coding agent wants to know how it does on **their** code:
their language mix, conventions, test setup and kind of bug.

Every repository with tests already holds such a benchmark in its history.
Each commit that changes code and adds a test that fails before it is a task
with a known-good answer and an objective check. Gitent can already find those
commits, because that is exactly `prove-pr`'s per-test proof (ADR 0017). It
can already drive any agent, because of the executors and presets (ADR 0004).

Our own evals taught us two things:

* the real-backport benchmark (`evals/real_backports.py`) only discriminated
  once the hidden tests were *proven*;
* the number that mattered most was not the pass rate but the agents' **false
  claims of success**.

## Decision

Three subcommands. They are experimental and CLI-only: they run agents, so they
are not exposed over MCP or HTTP.

### `gitent bench mine [--range <revs>] [--limit N] [--max-files N] [--out bench.json]`

Walk the first-parent history (default: the last 300 commits of `HEAD`) and
keep each non-merge commit `C` with parent `P` where:

* `C` changes at least one code file, at most `--max-files` files (default 20),
  and adds or modifies at least one test;
* at least one of those tests is **proven**: it fails on `P` with `C`'s test
  files overlaid, and passes on `C`. The same machinery as `prove-pr`.

Each kept commit becomes a task:

| field | meaning |
|---|---|
| `id`, `commit`, `parent` | `C`'s short sha, `C`, `P` |
| `statement` | `C`'s message with trailers removed (`Signed-off-by`, `Co-Authored-By`, `Gitent-*`, `Change-Id`, cherry-pick lines) |
| `hidden_tests` | the proven tests (name, file, and whether they were proven by assertion or only by a missing API) |
| `size` | files and lines changed in code, a difficulty proxy |

The output has `schema: gitent/bench@1` and records the test command and
selector it was mined with.

### `gitent bench run <bench.json> [--agent <preset>] [--tasks ids] [--keep] [--out results.json]`

For each task:

1. **An isolated workspace without the answer.** Write `P`'s tree into a fresh
   directory and create a *new* git repository there with a single commit. A
   worktree of the real repository would share its object database, so the
   agent could run `git show C`. Here `C` does not exist.
2. **The brief.** It contains the statement and the repository's test command:
   no diff, no hidden test names. Any executor can do the work. `--agent`
   picks a preset (`claude-code`, `codex`, `kiro`, `gemini`, `aider`);
   otherwise the configured executor is used (for example
   `--executor command --executor-command '…'`). The agent runs with the
   scrubbed environment of ADR 0016.
3. **Scoring.**
   * `C`'s test files are overlaid on the agent's result, replacing any tests
     the agent wrote in those files.
   * The hidden tests run with the usual runner rules. Exit 0 with failures
     reported, zero tests, or all tests skipped all count as failures.
   * **Resolved** means every hidden test passes.
   * A **false claim** means the agent reported success but the task is not
     resolved.

Results have `schema: gitent/bench-run@1`, with per-task results (resolved,
hidden test results, the agent's claim, time and diff size) and totals.

### `gitent bench compare <results.json>...`

A table with a row per task and a column per run, and totals for resolved,
false claims and median time. It is also available as Markdown.

## Consequences and limits

* **Statements are commit messages,** not issue reports. Commit messages
  often describe the fix as well as the problem, so they are easier than real
  issues. A later version can take PR or issue text from the forge, or have an
  LLM rewrite each statement as a bug report.
* **Mining runs tests** twice per candidate test. It is incremental by
  commit, and `--limit` caps it.
* **Old commits** are run with *today's* test command. Tasks whose tests
  cannot run that way are not proven and are skipped, so they cannot pollute
  the benchmark.
* **Contamination.** Public repositories may be in a model's training data.
  Isolation stops an agent from *looking up* the answer, not from *remembering*
  it. Recent commits are the most trustworthy tasks.
