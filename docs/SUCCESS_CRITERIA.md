# Success criteria

This document is the contract for "done". Each criterion has an ID, a concrete
check, and where it is verified. Agents working on Gitent must keep this file
truthful: a criterion is only marked ✅ when an automated check exists.

Legend: ✅ verified by automated test · 📊 verified by eval run (real LLM) · ⏳ not yet

## A. Foundation

| ID | Criterion | Verified by |
|----|-----------|-------------|
| A1 | `gitent init` works in a new or existing Git repo, writes `.gitent/config.toml`, and is idempotent. | ✅ `tests/cli_basic.rs::init_*` |
| A2 | Semantic records are persisted in `refs/notes/gitent`, keyed by Git commit, readable by `log`/`show`, and survive process restarts. | ✅ `tests/cli_basic.rs::commit_records_intent` |
| A3 | Every command supports `--json` with a stable, documented shape (`gitent schema`). | ✅ `tests/cli_basic.rs::json_output_*` |
| A4 | A Gitent repo stays a valid Git repo: `git log`, `git fsck` succeed; commits carry a `Gitent-Intent:` trailer. | ✅ `tests/cli_basic.rs::git_compat` |

## B. Semantic commit

| ID | Criterion | Verified by |
|----|-----------|-------------|
| B1 | `gitent commit` extracts a structured intention (title, intent, behaviour, symbols, constraints, acceptance, tests) from the **actual** diff via the configured LLM. | ✅ `tests/cli_basic.rs::commit_with_llm_analyzer` · 📊 evals |
| B2 | LLM claims are checked against the diff: unknown files are dropped, symbols not present in the diff are marked `verified=false`. | ✅ `tests/cli_basic.rs::analyzer_claims_are_checked`, unit tests in `analyze.rs` |
| B3 | Without an LLM (or if it fails) commit still succeeds with a deterministic heuristic record marked `analysis.method = "heuristic"`; `--require-llm` makes this an error. | ✅ `tests/cli_basic.rs::commit_heuristic_fallback` |
| B4 | A host agent can supply the intention itself (`--intent-file`, MCP `intent` argument); Gitent validates it against the diff. | ✅ `tests/cli_basic.rs::commit_with_host_intent`, `tests/mcp.rs` |

## C. Semantic cherry-pick (first vertical slice)

| ID | Criterion | Verified by |
|----|-----------|-------------|
| C1 | When the patch applies cleanly **and** validation passes, Gitent uses the deterministic result and does not call the agent. | ✅ `tests/cherry_pick.rs::clean_pick_uses_git` |
| C2 | When `git cherry-pick` conflicts because the target renamed/moved/restructured code, Gitent re-implements the intention with an agent and the result passes the target's tests **and** the tests carried by the original change. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent` · 📊 evals E1, E2 |
| C3 | When the patch applies cleanly but is **semantically wrong** (tests fail), Gitent detects it and escalates to the agent instead of committing. | ✅ `tests/cherry_pick.rs::clean_but_wrong_escalates` · 📊 eval E3 |
| C4 | Agent output that fails validation is retried with feedback up to `max_attempts`, then **rejected**: branch untouched, worktree kept for inspection, operation marked failed. | ✅ `tests/cherry_pick.rs::bad_agent_rejected` |
| C5 | Guards: an agent cannot pass by deleting/renaming the source change's tests or pre-existing tests. | ✅ `tests/cherry_pick.rs::deleting_tests_is_rejected` |
| C6 | Refuses to start with a dirty working tree; never overwrites uncommitted changes; aborts if the branch moved during the operation. | ✅ `tests/cherry_pick.rs::dirty_tree_refused`, `branch_moved_aborts` |
| C7 | Provenance: the result commit's record links `derived_from` (source commit + intent id), shares the `intent_id`, and stores validation evidence and the executor used. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent` |
| C8 | Host mode: an external agent receives a task brief + worktree, edits, then `gitent continue` validates and commits (`gitent abort` cleans up). | ✅ `tests/cherry_pick.rs::host_mode_roundtrip`, `tests/mcp.rs` |

## D. Merge, rebase, revert

| ID | Criterion | Verified by |
|----|-----------|-------------|
| D1 | Merge: clean + validated → deterministic merge commit; conflicting → agent integrates both sides' intentions; result has two parents and a record composing both sides. | ✅ `tests/merge_rebase_revert.rs::merge_*` · 📊 eval E4 |
| D2 | Merge detects declared incompatibilities (LLM compatibility check) and stops with a report instead of guessing (override with `--force`). | ✅ `tests/merge_rebase_revert.rs::merge_incompatible_stops` |
| D3 | Rebase replays each intention onto the new base using the same engine; any failed step leaves the branch untouched. | ✅ `tests/merge_rebase_revert.rs::rebase_*` |
| D4 | Revert removes an intention's behaviour even after later refactors made `git revert` conflict, keeping later intentions' tests green. | ✅ `tests/merge_rebase_revert.rs::revert_*` · 📊 eval E5 |

## R. Requirements (ADR 0008, `docs/REQUIREMENTS.md`)

| ID | Criterion | Verified by |
|----|-----------|-------------|
| R1 | `gitent commit` stores requirements with stable `req_` ids from: every added test (auto), `--require/--test/--check` and intent drafts (author), and the analyzer (llm). | ⏳ `tests/requirements.rs::commit_collects_requirements` |
| R2 | LLM-proposed requirements become executable only if their test exists in the diff; LLM-proposed commands are never executed (stored as `judged`). | ⏳ `tests/requirements.rs::llm_requirements_are_checked` |
| R3 | Proof: each executable requirement fails on the parent (test files overlaid) and passes on the commit; non-discriminating checks are flagged (`prove=warn`) or rejected (`prove=require`); a failing `after` rejects the commit. | ⏳ `tests/requirements.rs::proof_*` |
| R4 | Ledger: live requirements = introduced in ancestry − retired − reverted; merges union both parents; cherry-picked copies keep their id and count once. | ⏳ unit tests in `requirements.rs`, `tests/requirements.rs::ledger_*` |
| R5 | `gitent check [rev]` runs the ledger, names the intention that introduced each failing requirement, exits 2 on failure. | ⏳ `tests/requirements.rs::check_reports_regression` |
| R6 | Cherry-pick/rebase must satisfy the source's requirements (re-bound by test name) and the target's ledger; the brief lists them as the definition of done. | ⏳ `tests/cherry_pick.rs::requirements_drive_pick` |
| R7 | Merge must satisfy both ledgers; the compatibility check compares requirement statements. | ⏳ `tests/merge_rebase_revert.rs::merge_requirements` |
| R8 | Revert retires the reverted intention's requirements; warns `revert_incomplete` if their checks still pass. | ⏳ `tests/merge_rebase_revert.rs::revert_retires_requirements` |
| R9 | `--retire <id> "<reason>"` requires a reason, is shown in log/show; `--check-ledger` rejects a commit that breaks an unretired live requirement. | ⏳ `tests/requirements.rs::retire_*` |
| R10 | `@1` records are read with the documented mapping (acceptance/constraints → judged, tests → test). | ⏳ unit test in `model.rs` |

## E. Agent integration

| ID | Criterion | Verified by |
|----|-----------|-------------|
| E1 | Executors: `builtin` (Gitent's own tool loop over any provider), `command` (any CLI coding agent; presets for claude-code, codex, kiro, gemini, aider), `host` (calling agent). | ✅ tests use `command` executors with real processes, `builtin` with the scripted provider |
| E2 | Providers: `anthropic`, `openai` (any OpenAI-compatible endpoint, e.g. Ollama), `command` (any CLI), `scripted` (replay), `none`. Core has no vendor dependency. | ✅ unit tests in `llm/`; `command`/`scripted` in integration tests |
| E3 | MCP server over stdio exposes the operations as tools (`initialize`, `tools/list`, `tools/call`). | ✅ `tests/mcp.rs` |
| E4 | HTTP API exposes the same tool surface (`POST /v1/tools/<name>`). | ✅ `tests/mcp.rs::http_api` |
| E5 | Ready-to-use integration files for Claude Code, Codex, Kiro (MCP config + agent instructions). | `integrations/` (documentation) |

## F. The headline claim (comparative evaluation)

> Demonstrate scenarios where traditional Git cherry-pick or merge encounters
> conflicts (or silently produces wrong code) because the underlying code has
> changed, but Gitent successfully reconstructs the intended behaviour —
> compared against both traditional Git and a conventional LLM coding agent
> given the same context.

| ID | Criterion | Verified by |
|----|-----------|-------------|
| F1 | An eval suite of ≥ 5 scenarios (rename/move, signature change, clean-but-wrong, merge, revert-after-refactor), each with **hidden** acceptance tests the agents never see. | `evals/scenarios/` |
| F2 | A runner executes three arms on each scenario — `git` (plain git op), `baseline` (same coding agent, given the diff + message, no Gitent), `gitent` — and scores each against the hidden tests. | `evals/run.sh` |
| F3 | Gitent ≥ baseline ≥ git on scenario pass rate, and Gitent never reports success on a result that fails hidden tests (no false "success"). | 📊 `evals/RESULTS.md` |

The eval measures real agent behaviour and is therefore not part of `cargo
test` (it costs money and is non-deterministic). Results are committed with the
date, agent, and model used.
