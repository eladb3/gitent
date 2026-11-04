# Changelog

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
