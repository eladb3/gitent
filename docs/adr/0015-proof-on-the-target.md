# 0015: Prove the ported test on the target

**Status:** accepted. **Amends:** 0013. **Prompted by:** the external re-evaluation of
2026-11-07, round 2.

## Context

Round 1 (ADR 0013) closed every false accept by *enumerating* ways a test can be
neutered: skip markers, fewer assertions, changed literals. Round 2 showed two
problems with that.

* **Enumeration does not end.** Each of these still came back verified:
  * B2: `assertEqual` became `assertLessEqual`, with every literal kept;
  * B3: an early `return` as the test's first line;
  * B4: `try: … except AssertionError: pass`;
  * X2: `tests/__init__.py` forcing exit code 0;
  * code review listed a dozen more, such as `if False:`, overriding
    `assertEqual`, `@expectedFailure` and `conftest.py` hooks.
* **The literal heuristic punishes good ports.** It compares *every* literal on
  an assertion line, inputs included. The canonical good backport changes how
  the call looks (`Cart([(100, 1)]).discounted(150)` becomes
  `apply_discount(100, 150)`), loses the literal `1`, and came back
  `unverified`. By default, the Action fails it.

`gitent commit` already avoids both problems for the *source* change. It proves
each requirement: the test must fail on the parent with the commit's test files
overlaid, and pass on the commit. A test that cannot fail proves nothing,
however it was neutered.

## Decision

### 1. `ported_test_discriminates` (definitive)

When a move is validated (by `verify`, `verify-pr` or an operation step), take
the target *before* the move (`onto`), overlay the result's test files, and run
each executable requirement of the moved change:

* **passes on `onto` + the result's tests** → **rejected**: "the ported test
  passes without the port; it cannot tell fixed from unfixed";
* **fails there, passes on the result** → proven;
* cannot be run there (no selector, no test command) → no proof, and `evidence`
  decides (§3).

A result identical to `onto` (an already-realised change) is not proved; that
case is handled as before.

This catches B, B2, B3, B4, K and X2, plus skip, control-flow and harness
tricks, without listing them. A test that cannot fail also passes on the
unfixed target.

### 2. The heuristics become advisory

The assertion-count and literal comparisons no longer change the verdict for
the moved change's own tests. They appear as notes in `tests_intact`'s detail.
These still count:

* **Skip/ignore/xfail/expected-failure markers** added to a test: rejected.
  The regex is tightened (`Skip\s*=` matched `rowsToSkip = 3`). It now also
  covers `@skip`, `@expectedFailure`, `raise SkipTest`, `importorskip`,
  `it.failing` and `#[should_panic]`.
* **A target-only test the move weakened** (fewer assertions, changed
  literals): review. The proof covers the moved change's tests, not the
  target's.

### 3. `evidence` counts executions

A requirement counts as evidence only if it *ran* and passed. An all-skipped
run, or one with no test command, is `unverified`.

### 4. Runner output is read, not just the exit code

A run passes only if it exits 0, does not report failures in its summary, and
did not run zero tests.

* **Failure summaries:** `FAILED (failures=…)`, `N failed`, `Tests: N failed`,
  `--- FAIL`, `test result: FAILED`.
* **Zero tests ran:** this now includes all-skipped runs (unittest
  `OK (skipped=N)` with `Ran N`, pytest `N skipped` with nothing passed, vitest
  `Tests  N skipped`).
* **Go:** `[no tests to run]` counts only when *no* package ran a test, so
  multi-package modules are no longer rejected.
* **cargo:** detection reads all of stdout, not a 4000-character tail.

### 5. Harness files (`harness_unchanged`, review)

A move that changes a file able to change *how tests run* or what they exit
with is a review failure, unless the moved change itself changed that file:

* `conftest.py` and `__init__.py` under test directories;
* `pytest.ini`, `tox.ini`, `setup.cfg`, the `[tool.pytest` section of
  `pyproject.toml`;
* `jest.config*`, `vitest.config*`, `.mocharc*`, and the `scripts`/`jest`
  keys of `package.json`;
* `func TestMain(`;
* `.cargo/config*`;
* `.gitent/config.toml`.

### 6. Smaller fixes

* **Selector placeholders** are expanded in one pass, so a test name containing
  `{file}` stays inside its quotes. A new `{test_esc}` placeholder
  (regex-escaped, no anchors) is the default for jest, vitest and mocha.
* **Stub detection** counts a one-line body on the header line
  (`def f(x): return x * 2`).
* **`no_orphan_copy`** also flags a port that modifies *no* existing target
  code file when the source modified one. That covers copying to
  `cart_v2.py`, not just to the same path.
* **verify-pr** refuses a source commit that is already on the base branch.
* **The LLM verifier CLI** runs in an empty temporary directory, with the diff
  and the changed files in the prompt. The tree it judges can contain
  `.claude/settings.json`, `.mcp.json` or `AGENTS.md`, and these must not
  configure it.
* **Markdown:** commit subjects, warnings and verifier concerns are escaped
  before they go into the PR comment, including `@` mentions.

## Consequences

* A proof costs one more selector run per requirement, on a second temporary
  worktree.
* Port A (adapted call shape) and port H (test in another file) are verified
  again.
* Some ports stay `unverified` rather than rejected, because only a human can
  judge them:
  * J, which copies dead code: its test fails on `onto`, because the copied
    module does not exist there;
  * L, which weakens a target-only test;
  * T, a fix with no test.
* F, which hard-codes the tested inputs, remains the oracle limit.
