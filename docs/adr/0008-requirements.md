# 0008: Requirements are first-class, checkable parts of a commit

**Status:** accepted (design). Implementation is tracked in `docs/SUCCESS_CRITERIA.md` § R.
**Amends:** 0006 (validation policy). **Spec:** `docs/REQUIREMENTS.md`.

## Context

Until now an intention carried free-text `acceptance` and `constraints`, plus
a list of test names it introduced. Validation ran one global test command.
That has three problems:

1. "Re-implement this intention" is vague. Agents and the verifier have to guess
   what "done" means.
2. A passing global test suite does not show that *this* change's behaviour is
   present (the change's tests could be missing, or could test nothing).
3. Behaviour silently regresses later: nothing records that a past commit
   promised "lookups are cached", so nothing notices when that stops being true.

## Decision

Every semantic commit carries a list of **requirements**. Each requirement is a
statement plus, where possible, an executable **check**:

| kind | check | trust |
|---|---|---|
| `test` | a test identifier, run through the repo's `test_selector` template | strong |
| `command` | a shell command with an expected exit status | strong |
| `judged` | none: the statement is evaluated by the LLM verifier | weak, always labelled |

Rules:

* **Every test added by a commit becomes a `test` requirement automatically.**
  Recording requirements costs nothing by default.
* **Proof at commit time.** Each executable requirement must *fail on the parent*
  (with the commit's test files overlaid) and *pass on the commit*. Checks that
  already pass on the parent are flagged as `non_discriminating`.
* **Ledger.** The *live* requirements at a commit are all requirements
  introduced in its ancestry, minus those retired or reverted. Merges take the
  union of both parents.
* **Operations satisfy requirements**, not prose:
  * cherry-pick and rebase must satisfy the source's requirements *and* the
    target's live ledger;
  * merge must satisfy both sides' ledgers;
  * revert retires the reverted intention's requirements.
* **Behaviour changes are explicit.** A requirement leaves the ledger only by
  being *retired with a reason* (`--retire <id> "<reason>"`) or by a revert.
  `gitent check` reports any live requirement that fails, naming the intention
  that introduced it.
* **LLM output stays untrusted.** The analyzer may *propose* requirements. A
  proposal becomes executable only if it references a test that exists in the
  diff (checked deterministically); otherwise it is stored as `judged`.
* **Requirements point at the project's own tests.** Gitent does not become a
  test framework.

## Consequences

* Record schema bumps to `gitent/semantic-change@2`. `acceptance`,
  `constraints` and `tests` from `@1` records are read as `judged`,
  `judged`/constraint, and `test` requirements respectively.
* Validation (ADR 0006) gains a `requirements` check that supersedes the
  coarse `source_tests_present` check. The global `test_command` still runs as a
  regression gate.
* Portability: `test` requirements are re-bound **by test name** on the target,
  so they survive renames and moves. `command` checks are literal; if one
  cannot run on the target, the operation fails rather than guessing.
* Honest limit: Gitent can prove that a ported test keeps its name, passes,
  and fails on the target without the change. It cannot prove the ported test
  asserts *the same thing* as the original. That residual is checked by the
  verifier and reported as `judged`.
* Cost: computing the ledger walks history notes (an index is a later
  optimisation). Running the full ledger is opt-in at commit time and is the
  default for `gitent check` and for operations.
