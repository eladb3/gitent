# Success criteria

This document is the contract for "done". Each criterion has an ID, a concrete
check, and where it is verified. Agents working on Gitent must keep this file
truthful: a criterion is only marked ✅ when an automated check exists.

Legend: ✅ verified by automated test · 📊 verified by eval run (real LLM) · ⏳ not yet

## A. Foundation

| ID | Criterion | Verified by |
|----|-----------|-------------|
| A1 | `gitent init` works in a new or existing Git repo, writes `.gitent/config.toml`, and is idempotent. | ✅ `tests/cli_basic.rs::init_creates_config_and_is_idempotent` |
| A2 | Semantic records are persisted in `refs/notes/gitent`, keyed by Git commit, readable by `log`/`show`, and survive process restarts. | ✅ `tests/cli_basic.rs::commit_records_intent_and_requirements` |
| A3 | Every command supports `--json` with a stable, documented shape (`gitent schema`). | ✅ `tests/cli_basic.rs::json_output_and_schema` |
| A4 | A Gitent repo stays a valid Git repo: `git log`, `git fsck` succeed; commits carry a `Gitent-Intent:` trailer. | ✅ `tests/cli_basic.rs::git_compat` |

## B. Semantic commit

| ID | Criterion | Verified by |
|----|-----------|-------------|
| B1 | `gitent commit` extracts a structured intention (title, intent, behaviour, symbols, constraints, acceptance, tests) from the **actual** diff via the configured LLM. | ✅ `tests/cli_basic.rs::commit_with_llm_analyzer_claims_are_checked` (stand-in LLM) · 📊 evals ⏳ |
| B2 | LLM claims are checked against the diff: unknown files are dropped, symbols not present in the diff are marked `verified=false`. | ✅ `tests/cli_basic.rs::commit_with_llm_analyzer_claims_are_checked`, `analyze::tests::claims_are_checked` |
| B3 | Without an LLM (or if it fails) commit still succeeds with a deterministic heuristic record marked `analysis.method = "heuristic"`; `--require-llm` makes this an error. | ✅ `tests/cli_basic.rs::commit_heuristic_fallback_and_require_llm` |
| B4 | A host agent can supply the intention itself (`--intent-file`, MCP `intent` argument); Gitent validates it against the diff. | ✅ `tests/cli_basic.rs::commit_with_host_intent`, `tests/mcp.rs::mcp_stdio_end_to_end` |

## C. Semantic cherry-pick (first vertical slice)

| ID | Criterion | Verified by |
|----|-----------|-------------|
| C1 | When the patch applies cleanly **and** validation passes, Gitent uses the deterministic result and does not call the agent. | ✅ `tests/cherry_pick.rs::clean_pick_uses_git_and_no_agent` |
| C2 | When `git cherry-pick` conflicts because the target renamed/moved/restructured code, Gitent re-implements the intention with an agent and the result passes the target's tests **and** the tests carried by the original change. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent_with_provenance` (stand-in agent) · 📊 evals ⏳ |
| C3 | When the patch applies cleanly but is **semantically wrong** (tests fail), Gitent detects it and escalates to the agent instead of committing. | ✅ `tests/cherry_pick.rs::clean_but_wrong_pick_escalates_to_agent` · 📊 evals ⏳ |
| C4 | Agent output that fails validation is retried with feedback up to `max_attempts`, then **rejected**: branch untouched, worktree kept for inspection, operation marked failed. | ✅ `tests/cherry_pick.rs::bad_agent_is_retried_then_rejected` |
| C5 | Guards: an agent cannot pass by deleting/renaming the source change's tests or pre-existing tests. | ✅ `tests/cherry_pick.rs::deleting_tests_is_rejected` |
| C10 | A result that resurrects paths the target deleted, or drops definitions the target has (beyond what the moved change does), is rejected (ADR 0010). | ✅ `tests/cherry_pick.rs::stale_port_resurrecting_deleted_module_is_rejected`, `dropping_target_definitions_is_rejected` |
| C11 | A step whose change is already realised (no diff, source requirements pass) is skipped like git's empty commit, not failed. | ✅ `tests/cherry_pick.rs::already_realised_change_is_skipped_not_failed` |
| C9 | On a textual conflict the agent starts from all non-conflicting parts already applied (conflicting files as on the target). | ✅ `tests/merge_rebase_revert.rs::merge_conflict_composes_both_intentions` |
| C6 | Refuses to start with a dirty working tree; never overwrites uncommitted changes; aborts if the branch moved during the operation. | ✅ `tests/cherry_pick.rs::dirty_tree_is_refused`, `branch_moved_during_operation_aborts` |
| C7 | Provenance: the result commit's record links `derived_from` (source commit + intent id), shares the `intent_id`, and stores validation evidence and the executor used. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent_with_provenance` |
| C8 | Host mode: an external agent receives a task brief + worktree, edits, then `gitent continue` validates and commits (`gitent abort` cleans up). | ✅ `tests/cherry_pick.rs::host_mode_roundtrip`, `tests/mcp.rs::mcp_stdio_end_to_end` |

## D. Merge, rebase, revert

| ID | Criterion | Verified by |
|----|-----------|-------------|
| D1 | Merge: clean + validated → deterministic merge commit; conflicting → agent integrates both sides' intentions; result has two parents and a record composing both sides. | ✅ `tests/merge_rebase_revert.rs::merge_clean_is_deterministic`, `merge_conflict_composes_both_intentions` · 📊 evals ⏳ |
| D2 | Merge detects declared incompatibilities (LLM compatibility check) and stops with a report instead of guessing (override with `--force`). | ✅ `tests/merge_rebase_revert.rs::merge_incompatible_intentions_stop` |
| D3 | Rebase replays each intention onto the new base using the same engine; any failed step leaves the branch untouched. | ✅ `tests/merge_rebase_revert.rs::rebase_replays_intentions_and_failure_leaves_branch` |
| D4 | Revert removes an intention's behaviour even after later refactors made `git revert` conflict, keeping later intentions' tests green. | ✅ `tests/merge_rebase_revert.rs::revert_after_refactor_retires_requirements` · 📊 evals ⏳ |

## R. Requirements (ADR 0008, `docs/REQUIREMENTS.md`)

| ID | Criterion | Verified by |
|----|-----------|-------------|
| R1 | `gitent commit` stores requirements with stable `req_` ids from: every added test (auto), `--require/--test/--check` and intent drafts (author), and the analyzer (llm). | ✅ `tests/cli_basic.rs::commit_records_intent_and_requirements`, `tests/requirements.rs::author_requirements_from_flags` |
| R2 | LLM-proposed requirements become executable only if their test exists in the diff; LLM-proposed commands are never executed (stored as `judged`). | ✅ `tests/cli_basic.rs::commit_with_llm_analyzer_claims_are_checked`, `requirements::tests::collects_auto_author_and_checks_llm` |
| R3 | Proof: each executable requirement fails on the parent (test files overlaid) and passes on the commit; non-discriminating checks are flagged (`prove=warn`) or rejected (`prove=require`); a failing `after` rejects the commit. | ✅ `tests/requirements.rs::proof_rejects_requirement_failing_after_change`, `proof_flags_or_rejects_non_discriminating_checks` |
| R4 | Ledger: live requirements = introduced in ancestry − retired − reverted; merges union both parents; cherry-picked copies keep their id and count once. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent_with_provenance` (id kept, counted once), `tests/merge_rebase_revert.rs::merge_clean_is_deterministic` (union) |
| R5 | `gitent check [rev]` runs the ledger, names the intention that introduced each failing requirement, exits 2 on failure. | ✅ `tests/requirements.rs::check_reports_regression_and_names_intention` |
| R6 | Cherry-pick/rebase must satisfy the source's requirements (re-bound by test name) and the target's ledger; the brief lists them as the definition of done. | ✅ `tests/cherry_pick.rs::conflict_reimplemented_by_agent_with_provenance`, `host_mode_roundtrip` (brief), `clean_but_wrong_pick_escalates_to_agent` |
| R7 | Merge must satisfy both ledgers; the compatibility check compares requirement statements. | ✅ `tests/merge_rebase_revert.rs::merge_conflict_composes_both_intentions` |
| R8 | Revert retires the reverted intention's requirements; a revert whose reverted requirements still pass is rejected (`revert_effective`). | ✅ `tests/merge_rebase_revert.rs::revert_after_refactor_retires_requirements`, `revert_that_keeps_the_behaviour_is_rejected` |
| R9 | `--retire <id> "<reason>"` requires a reason, is shown in log/show; `--check-ledger` rejects a commit that breaks an unretired live requirement. | ✅ `tests/requirements.rs::retire_and_check_ledger` (also: editing a test bound to a live requirement is rejected under `--check-ledger`) |
| R10 | `@1` records are read with the documented mapping (acceptance/constraints → judged, tests → test). | ✅ `model::tests::migrates_v1_records` |

## V. Verify (ADR 0011)

| ID | Criterion | Verified by |
|----|-----------|-------------|
| V1 | `gitent verify <source> [<result>]` validates an existing commit as a move of `<source>` onto `<result>^`, with the same checks as an operation step; exit 0 verified, 2 rejected. | ⏳ `tests/verify.rs::verify_good_and_bad_backports` |
| V2 | `--onto <base>` verifies a multi-commit result as a whole. | ⏳ `tests/verify.rs::verify_multi_commit_result` |
| V3 | `--working-tree` verifies uncommitted changes against `HEAD` without modifying the user's index or files. | ⏳ `tests/verify.rs::verify_working_tree_is_side_effect_free` |
| V4 | `--kind revert` and `--kind merge` apply `revert_effective` and both-sides preservation. | ⏳ `tests/verify.rs::verify_revert_and_merge` |
| V5 | `--record` attaches a semantic record (operation `verify`, `derived_from`, validation) only when verified. | ⏳ `tests/verify.rs::verify_record` |
| V6 | Available over MCP (`gitent_verify`) and HTTP. | ⏳ `tests/mcp.rs` |
| V7 | Operations and verify share one validation code path (`validate_candidate`). | ⏳ code review + all operation tests still green |
| V8 | Eval: `gitent verify` as a gate behind the *baseline* agent reaches the same false-success rate as Gitent operations (0), at no extra agent cost. | ⏳ `evals/run.py --arms baseline,baseline+verify` |

## E. Agent integration

| ID | Criterion | Verified by |
|----|-----------|-------------|
| E1 | Executors: `builtin` (Gitent's own tool loop over any provider), `command` (any CLI coding agent; presets for claude-code, codex, kiro, gemini, aider), `host` (calling agent). | ✅ all operation tests use `command` executors (real processes); `tests/cherry_pick.rs::builtin_agent_with_scripted_provider`; `host` in `host_mode_roundtrip` |
| E2 | Providers: `anthropic`, `openai` (any OpenAI-compatible endpoint, e.g. Ollama), `command` (any CLI), `scripted` (replay), `none`. Core has no vendor dependency. | ✅ unit tests in `llm/`; `command`/`scripted` in integration tests (anthropic/openai: ⏳ no live test) |
| E3 | MCP server over stdio exposes the operations as tools (`initialize`, `tools/list`, `tools/call`). | ✅ `tests/mcp.rs::mcp_stdio_end_to_end` |
| E4 | HTTP API exposes the same tool surface (`POST /v1/tools/<name>`). | ✅ `tests/mcp.rs::http_api` |
| E5 | Ready-to-use integration files for Claude Code, Codex, Kiro (MCP config + agent instructions). | ✅ files in `integrations/` (not exercised against the real CLIs ⏳) |

## F. The headline claim (comparative evaluation)

> Demonstrate scenarios where traditional Git cherry-pick or merge encounters
> conflicts (or silently produces wrong code) because the underlying code has
> changed, but Gitent successfully reconstructs the intended behaviour —
> compared against both traditional Git and a conventional LLM coding agent
> given the same context.

| ID | Criterion | Verified by |
|----|-----------|-------------|
| F1 | An eval suite of ≥ 5 scenarios (rename/move, signature change, clean-but-wrong, merge, revert-after-refactor), each with **hidden** acceptance tests the agents never see. | ✅ 9 scenarios in `evals/cases.py` (+ parity control), validated by `--agent offline` |
| F2 | A runner executes three arms on each scenario — `git` (plain git op), `baseline` (same coding agent, given the diff + message, no Gitent), `gitent` — and scores each against the hidden tests. | ✅ `evals/run.py` (also: fault-injection agent `--agent stale`) |
| F3 | Gitent ≥ baseline ≥ git on scenario pass rate, and Gitent never reports success on a result that fails hidden tests (no false "success"). | 📊 met in `evals/RESULTS.md` run 7: git 1/9, baseline 9/9, gitent 9/9, 0 false successes; fault injection: baseline 8 false successes, gitent 0. **Gitent ties a capable agent; it does not (yet) beat it.** |
| F4 | Scenarios hard enough that a capable agent alone fails some, to test whether Gitent improves correctness (not just safety). | 📊 tested on 24 real Django backports (19 git conflicts): agent alone 24/24, gitent 24/24. **Not demonstrated**: no case where Gitent made a capable agent more correct. Fault injection on the same cases: baseline 9 false successes, gitent 0. |

The eval measures real agent behaviour and is therefore not part of `cargo
test` (it costs money and is non-deterministic). Results are committed with the
date, agent, and model used.
