# Changelog


## Unreleased

Addresses the external hands-on evaluation of 2026-11-07 (ADR 0013, ADR 0014).

### Verification integrity
- **Three verdicts:** `verified` (exit 0), `rejected` (2) and `unverified` (3, needs review). `verify` and `verify-pr` JSON now carry `schema` and `verdict`.
- **New checks:** `tests_intact` (skipped, weakened or rewritten tests), `bodies_intact` (definitions stubbed out), `no_orphan_copy` (source-branch code copied over), and `evidence` (nothing executable proved the move).
- **Qualified test identity:** requirements record `test_path`, and records are now `gitent/semantic-change@3`. A selector run that executes 0 tests fails, and placeholders are shell-quoted (`{test}`, `{test_re}`, `{file}`, `{module}`).
- **Reverts:** a revert must make the reverted requirements fail with their tests present; deleting the test is not enough.
- **`verify-pr`:**
  - checks the whole PR for every source and reads every marker;
  - refuses a PR that cites its own commits;
  - reads config from the base and writes no notes;
  - adds `--trust-notes` and a positional base;
  - renders for a terminal on a TTY.
- **Ledger:** the newest event wins, and merges keep retirements.
- **LLM verifier:** runs in the result tree.
- **Fault models:** `tests/fault_models.rs` covers 16 scenarios.

### Other
- `init` detects pytest, vitest and mocha, and its selectors run exactly one test.
- The `Gitent-Intent:` trailer is optional (`[commit] intent_trailer = false`).
- Experimental subcommands are labelled in `--help`.

## v0.1.0 (unreleased)

First release. Gitent is a Git companion that records what each change is for
and what it must keep doing, and verifies that changes moved between diverged
branches still do it.

* **`gitent verify`**: checks that a commit, branch or working tree correctly
  moves a change. Kinds: apply, revert or merge. No agent is involved.
* **`gitent verify-pr`** and the **GitHub Action** (`uses: eladb3/gitent@v0`):
  finds backports, cherry-picks and reverts from commit markers, verifies them,
  writes a step summary, keeps a sticky PR comment, and fails on rejection.
* **Semantic commits with requirements.** Each requirement is proven at commit
  time (fails before the change, passes after it). They accumulate into a
  ledger, checked with `gitent check`.
* **Operations**: `cherry-pick`, `merge`, `rebase` and `revert`. Each tries Git
  first and validates the result. Otherwise it drives any coding agent (Claude
  Code, Codex, Kiro, Gemini, aider, built-in, or host mode) in an isolated
  worktree.
* **Interfaces**: MCP server, HTTP API, and `--json` on every command.
* **Evidence**: see `evals/RESULTS.md`, including the limits.
