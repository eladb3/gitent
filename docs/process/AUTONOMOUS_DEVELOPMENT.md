# Plan: autonomous development of Gitent, with Git as ground truth

**Status:** proposed (plan only; nothing below is built or scheduled yet).

Gitent is built by agents. This plan sets up the infrastructure that lets Claude
routines (scheduled Claude Code sessions) build Gitent continuously and safely.
Plain **Git is the ground truth** ("the oracle") that every result is measured
against.

Decisions already taken:

| Decision | Choice |
|---|---|
| Merge policy | **Auto-merge** when CI is green (including the parity and safety gates) *and* the reviewer routine approves |
| Live eval (real coding agent, costs tokens) | **Nightly** |
| Parallel builders | **1** to start; add lanes later |

---

## 1. Git as the oracle

### 1.1 The three tiers

Every eval case is a small repository history plus one operation
(`cherry-pick`, `merge`, `rebase`, `revert`, `commit`). The runner performs the
operation with **plain git** and with **Gitent**, then classifies the case by
what git did:

| Tier | When | Requirement for Gitent | Gate |
|---|---|---|---|
| **Parity** | git succeeds *and* git's result passes the hidden tests | **Byte-identical tree OID** to git's result, `provenance.executor == "git"`, zero LLM/agent calls | hard: 100 % |
| **Recovery** | git conflicts, *or* git "succeeds" but its result fails the hidden tests | result passes the hidden tests | ratchet: pass rate never decreases |
| **Safety** | all cases | never report `succeeded` on a result that fails hidden tests ("false success"); never modify a dirty working tree; branch unchanged on failure | hard: 0 violations |

Parity also covers the non-operation commands:

* `gitent commit` tree == `git add -A && git commit` tree;
* `gitent log` commit order == `git log --topo-order`;
* `gitent diff A B` file set == `git diff --name-status A B`;
* the repository passes `git fsck` after every operation.

Git therefore defines correct behaviour wherever it works. Gitent's job is to
match it exactly there, and to beat it only where it fails.

### 1.2 Case format (`eval/cases/<id>/`)

```
case.toml        # id, tier hint, operation, branch names, tags (rename, move, signature, units, …)
setup.sh         # builds the repo deterministically in $1 (fixed author/date env)
hidden/          # acceptance tests copied in ONLY at scoring time; never visible to agents
expected.toml    # recorded oracle facts: git exit status, git tree OID (if clean), hidden-test result on git's tree
```

`expected.toml` is **generated** by running git (`make eval-record`), never
hand-written. It is a protected path.

### 1.3 Generator (`tools/gitent-gen`)

The generator produces cases as *base project × divergence mutator × change
template*:

* **Base projects:** small Python, Rust, JS and Go code bases with unit tests.
* **Divergence mutators** (applied on the target branch): rename symbol, move
  file, change signature, add a parameter, change units, reorder functions,
  reformat, split a function, extract a module, delete dead code.
* **Change templates** (applied on the source branch): add caching, add
  validation, add a parameter with a default, fix an off-by-one, add logging,
  change an error type, each with its own tests.
* **Hidden tests** come from the change template, adapted by the mutator
  (deterministically, so the oracle stays exact).

The mutator mix decides the tier. A mutator far from the change gives parity
cases; an overlapping one gives recovery cases. Every case is reproducible from
`(seed, base, mutator, template)`.

### 1.4 Runner (`gitent-eval`, built as a Rust bin target)

```
gitent-eval run   [--arms git,gitent,baseline] [--agent offline|claude-code|…] [--filter tag]
gitent-eval show  <case>          # per-arm result, diffs, hidden-test output
gitent-eval record                # (re)compute expected.toml from git
gitent-eval baseline              # update eval/baseline.json; refuses regressions
```

* **Arms:** `git` (oracle), `gitent`, and `baseline`. The baseline arm gives
  the same coding agent the source diff, the commit message and the target repo,
  with no Gitent involved. It only runs in live mode.
* **Agent modes:**
  * `offline`: deterministic stand-in agents in `eval/agents/`. Each case can
    ship a reference solution script, so every Gitent code path runs in CI with
    no tokens.
  * `live`: a real coding agent through Gitent's `command` executor.
* **Output:** `eval/runs/<timestamp>/report.json` and `report.md`, with
  `eval/runs/latest` pointing at the newest run. The report has per-tier and
  per-tag metrics for parity rate, recovery pass rate, false successes, LLM
  calls per case, wall time and tokens.
* **Ratchet:** `eval/baseline.json` holds the best accepted metrics per tier
  and tag. `make eval-gate` fails CI on any parity miss, any safety violation, or
  an offline recovery regression.

### 1.5 Progress dashboard

`make progress-report` renders `docs/progress.md` from the latest offline and
live runs. It shows tier rates over time, the worst tags, open failing cases and
cost per run. It is a generated file, protected, and refreshed only by the
steward routine.

---

## 2. Development process for agents

### 2.1 Tasks (`docs/tasks/T-XXXX-slug.md`)

```markdown
---
id: T-0042
title: Parity: rename-only divergence keeps deterministic path
status: ready            # draft | ready | in_progress | review | done | blocked
lane: engine             # engine | eval | integrations | docs
size: S                  # S | M  (L must be split)
depends: [T-0040]
criteria: [C1, F3]       # links to docs/SUCCESS_CRITERIA.md
---
## Context
## Acceptance criteria
- [ ] checkable statement (test name or eval case id)
## Outcome
(filled in when done: what changed, metrics before/after, follow-ups)
```

* `make next-task LANE=…` picks the first `ready` task in that lane whose
  dependencies are all `done`, ordered by id. `make board` renders a
  git-ignored `docs/tasks/BOARD.md`. `make new-task` scaffolds a task.
* Progress lives in each task's **Outcome** section, not in a shared log, so
  parallel agents don't conflict.
* Big or architectural work needs a spec first (`docs/specs/S-XXXX-*.md`),
  written as its own task. Decisions go in ADRs (`docs/adr/`, which already
  exists).

### 2.2 Protected paths and forbidden shortcuts

Changing these requires an ADR approved by the reviewer routine:

* `eval/cases/*/expected.toml`, `eval/baseline.json`, `docs/progress.md`;
* `docs/SUCCESS_CRITERIA.md` (adding criteria is fine; weakening one needs an ADR);
* CI gate definitions (`.github/workflows/`, `make check`).

Never allowed: deleting, skipping or weakening tests; blanket `#[allow]`;
`--allow-regressions`; editing a hidden test to match Gitent's output.

### 2.3 Quality gates

* `make check-fast` is the pre-push gate: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test`, and the offline
  eval on a quick subset.
* `make check` is the full CI gate: everything in `check-fast`, plus the full
  offline eval with `make eval-gate`, `cargo deny`, and a docs-link check.
* **CI** (`.github/workflows/ci.yml`) runs `make check` on every PR. It is the
  only merge gate together with the reviewer approval.

### 2.4 Skills and agents in `.claude/`

| Skill / agent | Purpose |
|---|---|
| `work-loop` skill | pick a task → branch `agent/<lane>/T-XXXX-slug` → write a failing test or eval case → implement → `make check-fast` → `review` → open a PR with commit message `T-XXXX: summary` → update the task's Outcome |
| `review` skill | runs parallel reviewer subagents (correctness, safety and policy against protected paths, test quality, docs drift) and posts one consolidated verdict |
| `steward` skill | runs the nightly live eval, refreshes `docs/progress.md`, files `ready` tasks for new failing cases and tags, and closes stale tasks |
| `git-probe` skill | when unsure how git behaves, build a minimal repo, run git, and record the observation as an eval case. **Never guess git semantics.** |
| `planner` agent | splits L tasks, keeps the backlog at ≥ 5 ready tasks per lane, and links tasks to criteria |
| hooks | block edits to protected paths without an ADR in the same branch; run `cargo fmt` after edits |

---

## 3. Claude routines

Each routine is a **scheduled task that starts a fresh Claude Code session** on
this repository, with a standalone prompt. They coordinate only through git
(branches, PRs, task files), never through shared session state.

| Routine | Cadence (UTC, jittered) | Prompt (summary) |
|---|---|---|
| **Builder** | every 3 h | "Run the `work-loop` skill for lane `engine` (fall back to any lane). Exactly one task, one PR. If no task is ready, run the `planner` agent instead." |
| **Reviewer/merger** | hourly | "For each open `agent/*` PR: run the `review` skill; if changes are requested, push fixes (never on protected paths without an ADR); if CI is green and review approves, merge. Never merge with a red parity or safety gate." |
| **Steward** | nightly, 02:xx | "Run `make eval-live` (Claude Code as the agent, git as the oracle, plus the baseline arm), then `make progress-report`; commit the report via PR; file tasks for regressions and new failures." |
| **Generator** | weekly | "Run `gitent-gen` for under-covered tags (from `docs/progress.md`), record the oracle with `make eval-record`, add the cases via PR, and file tasks for new failing cases." |

Guardrails:

* One builder at a time. Builders claim a task by setting
  `status: in_progress` in their first commit on the branch, and a task already
  claimed by an open PR is skipped.
* Every routine works on a branch and a PR, never directly on `main`. Only the
  reviewer/merger routine merges.
* Auto-merge requires all of: green CI, reviewer approval, no protected-path
  change without an approved ADR, and a non-decreasing recovery ratchet.
* Budget: the live eval is capped per night (case subset and timeout per case).
  The steward records token and cost totals in the report.
* Kill switch: disable the scheduled tasks. Unmerged branches are harmless
  because Gitent's own rule applies to the process too: nothing reaches `main`
  without validation.

---

## 4. Bootstrap order

| Milestone | Delivers | Exit criteria |
|---|---|---|
| **P0 Process** | `docs/process/{agent-workflow,task-format,quality-gates}.md`, task template, `make next-task/board/new-task`, CI with `make check`, protected-path hook | CI green on `main`; `make next-task` returns a task |
| **P1 Oracle** | case format, `gitent-eval run/show/record/baseline` with `git` + `gitent` arms in offline mode, 10 hand-built seed cases (all 3 tiers), `make eval-gate` in CI | parity 100 %, safety 0 on the seed cases; the gate fails on a deliberately broken branch |
| **P2 Generator** | `gitent-gen` with 3 bases × 6 mutators × 4 templates, deterministic hidden tests | ≥ 100 recorded cases; generation is reproducible from a seed |
| **P3 Backlog** | tasks seeded from every ⏳ criterion in `SUCCESS_CRITERIA.md` and every failing case | ≥ 15 ready tasks across lanes |
| **P4 Live** | `--agent claude-code` + baseline arm, `make eval-live`, `docs/progress.md` | one manual live run published |
| **P5 Routines on** | the 4 scheduled tasks, at half cadence for the first week | a week of merged PRs with no gate bypass; then full cadence |

Step 0, before P0, is the work still pending in this session: the offline
integration test suite in `tests/`, which P1 builds on.

---

## 5. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Agents "pass" by weakening tests or the oracle | hidden tests outside agent view; protected paths; test-preservation guards; reviewer policy check |
| Oracle drift across git versions | pin the git version in CI; store the git version in `expected.toml`; re-record on upgrade via PR |
| Overfitting to generated cases | keep a held-out case set the generator's tags never feed back into; report it separately |
| Merge-queue churn between builder and reviewer | one builder; small S/M tasks; reviewer rebases or asks for a rebase |
| Cost of live runs | nightly subset, per-case timeout, cost in the report, offline mode for all gating |
| Auto-merge of a subtle regression | parity is a hard gate, recovery is ratcheted, safety must be 0; any revert is just another PR |
