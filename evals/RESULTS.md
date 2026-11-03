# Eval results

Date: 2026-11-02 · git 2.43.0 · agent: Claude Code CLI 2.1.316 (`claude -p`) ·
Raw data: `evals/results/run*.json` · Harness: `evals/run.py`, scenarios: `evals/cases.py`.

**Arms.** All three start from an identical, freshly built repository.

* `git`: the plain git operation.
* `baseline`: the coding agent alone. It gets the commit message(s), the diff and
  the repo, and reports `RESULT: SUCCESS|FAILURE`.
* `gitent`: Gitent driving the *same* agent as its executor.

**Scoring.** Each case is scored by **hidden tests** that no arm ever sees. A
*false success* is an arm reporting success on a result that fails the hidden
tests.

## `gitent verify` as a gate behind an agent (ADR 0011, criterion V8)

Arm `baseline+verify`: the baseline agent works alone, exactly as in the
`baseline` arm. Its uncommitted result is then checked with
`gitent verify <source> --working-tree --kind …` before it would be
accepted. There is no extra agent run and no LLM verifier.

| | baseline (agent's own report) | baseline + `gitent verify` |
|---|---|---|
| **Live, Claude Code, 24 real Django backports** (run real5) | 23/24 correct, **1 false success** | 23 accepted · **1 false success** · 1 false rejection (bug, fixed, see below) |
| **Fault injection, real** (stale port, run real6) | 9 false successes | **0 false successes**. 9 accepted, all correct. 6 rejected results pass the hidden tests but leave the visible suite red. |
| **Fault injection, synthetic** (9 scenarios) | 8 false successes | **0** |
| **Reference solutions, synthetic** (no wrong work) | 9/9 accepted | 9/9 accepted (0 false rejections) |
| verify cost (real, live run) | — | 17.9 s average per backport, no agent tokens |

**What the live run showed.**

* **The first real agent mistake in any of our runs.** On
  `stable-5.2.x-6ecd66e09a` (CVE-2026-84429), Claude Code's backport mishandles
  invalid RFC 2231 encodings. **`gitent verify` accepted it.**
  * The human backporter *added* a test for this 5.2-specific behaviour
    (`test_rfc2231_invalid_encoding`), and the original fix on `main` has no
    such test.
  * Nothing recorded with the source change covers it, so no check against the
    source's requirements can catch it.
  * This is a real limit: Gitent enforces what is recorded. Behaviour that only
    matters on the target branch has to be a requirement *of the target*, in
    its ledger, to be protected.
* **A false rejection that was a Gitent bug.** On `stable-6.0.x-972dbdd4f7`,
  the original fix itself deletes `test_create_request_error`. The correct port
  deletes it too, and `existing_tests_preserved` rejected it. Tests deleted by
  the change being moved are now allowed to disappear
  (`tests/verify.rs::tests_removed_by_the_source_change_may_disappear`).
  Re-run: verified and accepted.
* **Correction.** Earlier fault-injection write-ups counted this case as a
  correct "deleted a test" rejection. It was this bug.

**Conclusion.** Putting `gitent verify` behind any agent gives the same
protection against broken and stale results as running the agent through
Gitent. On real code that is 9 false successes reduced to 0. It needs no extra
agent cost and takes about 18 s per check. It does **not** catch mistakes that
nothing recorded describes.

## Real backports (Django), the main evidence

The benchmark is `evals/real_backports.py`, with 24 cases in `evals/real/django_cases.json`. Each case was
mined from Django's stable branches using the `Backport of <sha> from main` convention:

* **X** is the original fix on `main`. Every arm is asked to move it.
* **target** is the parent of the human backport.
* **Hidden tests** are the *human backport's* test changes.

A case is kept only if those tests fail on the target and pass on the human
backport. Each arm works in a clone that holds only `main` (the target) and
`source` (X), so the human backport is unreachable.

The 24 cases split into 19 conflicts under `git cherry-pick`, 1 that applies
cleanly but differs from the human backport, and 4 identical controls. They
were picked from 970 candidates; 442 were rejected (size, multiple parents,
non-discriminating tests, or an environment issue).

| | plain git | Claude Code alone | Gitent + Claude Code |
|---|---|---|---|
| conflict cases (19) | 0/19 | 19/19 | 19/19 |
| all 24 | 5/24 | 24/24 | 24/24 |
| false successes | 0 | 0 | 0 |
| wall time (sum) | ~5 s | 1538 s | 2897 s (1.9×) |
| agent invocations | 0 | 24 | 20 (0 on the 5 cases git already solves) |

**Fault injection on the same real cases** uses the "stale port" agent: it
restores X's files verbatim on the old branch, then claims success.

| | correct per hidden tests | false successes | results left with a red visible suite or deleted tests, but reported success |
|---|---|---|---|
| baseline | 15/24 | **9** | 6 (all accepted) |
| Gitent | 12/24 (after the removed-test fix) | **0** | 0 (all rejected: failing visible tests) |

The hidden-test scorer overlays the human's test files. It therefore accepts
6 stale results whose own test suite is red, and Gitent's rejections of those
are correct.

**Bugs this real-code run found in Gitent.** The `target_preserved` guard
flagged a "definition" named `does`. A Python docstring, `struct does not
contain…`, matched the Rust `struct` pattern. Definition detection is now
per-language by file extension (`diff::defs_in_line_for`) and ignores non-code
files.

**Verdict on criterion F4** ("does Gitent make a capable agent more correct?"):
**No, not on this benchmark.** Claude Code alone solved all 19 real conflicting
backports. Gitent's measurable value over the agent is:

1. **Zero false successes under faulty agents**, against 9 for the agent's own report.
2. **No agent call at all** where git's result validates (5/24).
3. **Recorded provenance and requirements.**

It costs about 1.9× the wall time.

## Summary (synthetic scenarios)

| | plain git | agent alone (baseline) | Gitent + same agent |
|---|---|---|---|
| Live, Claude Code (run 7, 9 cases) | 1/9 · 1 false success | **9/9** · 0 | **9/9** · 0 |
| Live, Claude Code on Haiku 5.5 (run 4) | 1/9 · 1 | 9/9 · 0 | 9/9 · 0 |
| Fault injection: "stale port" agent (run 6) | — | 1/9 · **8 false successes** | 1/9 · **0 false successes** |
| Wall time, run 7 (sum over 9 cases) | ~1 s | 212 s | 510 s (~2.4×) |
| Agent invocations, run 7 | 0 | 9 | 10 (0 on the parity case; 3 on the 3-step rebase) |

## What this shows (and what it does not)

1. **Against plain Git the claim holds.**
   * Git conflicted on 7 of the 8 diverged scenarios.
   * On the 8th (E3, money switched to cents) it applied cleanly and was
     **silently wrong**.
   * Gitent reconstructed the intended behaviour in all 8, and used Git's own
     result, with no agent call, where Git was right (E0).
2. **Against a capable agent, Gitent ties on correctness; it does not beat it.**
   Claude Code alone solved every scenario, even on the small Haiku model.
   These scenarios are within easy reach of current agents. The headline
   criterion F ("Gitent ≥ baseline") is met as a tie. We have **no evidence yet
   that Gitent makes a capable agent more correct**.
3. **Gitent's distinct value is safety and accountability, not extra capability.**
   With a faulty agent, the baseline reported success on 8 wrong results, and
   Gitent on 0. Gitent never moved a branch to an unvalidated result. Every
   result carries:
   * the source intention and its requirements;
   * the validation evidence;
   * the agent used.
4. **It costs time.** Gitent took about 2.4× the baseline's wall time. That time
   goes to the extra verifier LLM call, running the suite plus each requirement,
   proofs on the target, and briefs. The verifier is optional (`--llm`); the
   offline checks are cheap.

## Findings that changed Gitent

| run | finding | fix |
|---|---|---|
| 1 | Gitent 3/6 vs baseline 6/6. Claude Code **refuses to edit files under `.git/`**, where Gitent placed worktrees. The offline test suite could not see this, because stand-in agents don't care. | ADR 0009: worktrees outside `.git/` |
| offline | A 3-step rebase failed because a later step was already realised and produced "no change". | already-realised steps are skipped, like git's empty commits (C11) |
| 5 | Fault injection: Gitent let **4 of 8** stale ports through. Resurrected modules made the original tests pass; overwritten target code was not covered by any test. | ADR 0010: `target_preserved` guard. Run 6: 0 of 8. |
| design | A revert that left the feature in place passed with only a warning. | `revert_effective` is now a failing check |

## Runs

| run | config | git | baseline | gitent |
|---|---|---|---|---|
| 1 | Claude Code, worktrees in `.git/` (6 cases) | 1/6 | 6/6 | 3/6 |
| 2 | Claude Code, ADR 0009 (6 cases) | 1/6 | 6/6 | 6/6 |
| 3 | Claude Code (9 cases) | 1/9 | 9/9 | 9/9 |
| 4 | Claude Code on Haiku 5.5 (9 cases) | 1/9 | 9/9 | 9/9 |
| 5 | fault injection, before ADR 0010 | — | 1/9, 8 FS | 1/9, 4 FS |
| 6 | fault injection, with ADR 0010 | — | 1/9, 8 FS | 1/9, 0 FS |
| 7 | Claude Code, with ADR 0010 (9 cases) | 1/9 | 9/9 | 9/9 |

(FS = false successes. Each live run is a single sample; agent behaviour is
non-deterministic.)

## Next steps for the eval

* **Harder scenarios**, where capable agents actually fail: larger code bases,
  long rebase stacks, conflicting requirements, changes spread across many
  files. Without these, the comparison with an agent stays a tie.
* **Repeated runs** (n ≥ 5 per case) to measure variance.
* **More realistic fault models** than "stale port":
  * partial ports;
  * weakened tests;
  * an agent that over-claims.
* **Cost accounting** in tokens, not just wall time.
