# Changelog

## 0.1.0

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
* **Install**: a one-line installer for the release binaries (checked against
  their SHA-256), `cargo install gitent` / `cargo binstall gitent`, and a
  Claude Code plugin (`/plugin marketplace add eladb3/gitent`).
* **Evidence**: see `evals/RESULTS.md`, including the limits.

### Hardening before the release: two external reviews

#### Round 2

Addresses the external re-evaluation (ADR 0015, ADR 0016).

- **Proof on the target:** the moved change's requirements must fail on the target without the port. This catches weakened, short-circuited, swallowed and harness-forced tests without listing them.
- **Heuristics advisory:** the literal and assertion-count heuristics are now notes for the moved change's tests, so good ports with an adapted call shape are verified again.
- **Evidence:** only requirements that actually ran and passed count; no test command means `unverified`.
- **Runner summaries are read:** failures with exit 0, all-skipped runs, Go multi-package runs, cargo output.
- **`harness_unchanged`:** a new review check.
- **Placeholders:** expanded in one pass, with a new `{test_esc}`.
- **Fixes:** the skip regex (no more `rowsToSkip` false reject), one-line bodies, `cart_v2.py`-style copies, and backport sources already on the base.
- **LLM verifier:** runs in an empty directory. Markdown escaping is hardened.

**Trust boundaries (ADR 0016)**
- **Scrubbed child environments:** test commands, selectors and checks get an allowlist; agent/LLM CLIs also get their provider keys; GitHub tokens, `ACTIONS_*` and `$GITHUB_ENV`-style files never; `GITENT_PASS_ENV` passes extra variables.
- **Agent commands need consent:** command requirements from an agent or LLM report `skipped` unless `requirements.trust_agent_commands` (local config) or `--trust-agent-commands`.
- **MCP/HTTP read command keys from HEAD**, not the working tree.
- **`gitent trust`:** the shared config's `executor.command`, `llm.command` and `llm.base_url` are ignored until trusted (by content hash).
- **`serve`:** non-loopback binds need `--allow-remote` and an explicit `GITENT_TOKEN`; a generated token goes to `.git/gitent/serve-token`.
- **Warnings:** the `aider` preset and the `builtin` executor are flagged as unsandboxed.
- **Action:** isolated source build, no GitHub token in the verify step, Python run with `-I`, `persist-credentials: false` in examples.

#### Round 1

Addresses the external hands-on evaluation of 2026-11-07 (ADR 0013, ADR 0014).

**Verification integrity**
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

**Other**
- `init` detects pytest, vitest and mocha, and its selectors run exactly one test.
- The `Gitent-Intent:` trailer is optional (`[commit] intent_trailer = false`).
- Experimental subcommands are labelled in `--help`.
