# 0017: `gitent prove-pr`, prove the tests of any pull request

**Status:** accepted. **Builds on:** 0008 (proof at commit), 0013, 0015 (proof on the target),
0016 (scrubbed environments).

## Context

The sharpest lesson of the two external reviews: **a test proves a change only
if it can fail without it.** `gitent commit` proves this on the source, and
ADR 0015 proves it on the target of a backport. That second proof caught every
neutered test the reviewer wrote, without enumerating tricks.

Nothing about this is specific to backports. Every pull request that changes
code and adds tests makes the same implicit claim: *these tests check this
change*. AI agents make that claim constantly, and they often write tests that
cannot fail:

* asserts on the mock instead of the code;
* expected values copied from the current (buggy) output;
* tests of something adjacent to the change.

They also delete or skip tests that got in their way.

Most repositories rarely backport. Every repository has pull requests.

## Decision

Add `gitent prove-pr`, which needs no notes, no `gitent commit` and no
markers:

```
gitent prove-pr [BASE] [--head <rev>] [--working-tree] [--require-all] [--markdown <file>]
```

1. **Scope.** The PR is `onto..head`, where `onto = merge-base(BASE, head)`.
   With `--working-tree`, the result is a snapshot of uncommitted changes
   against `HEAD`, the case where an agent checks its own work before
   committing.
2. **Tests to prove.** Every test the PR *adds* or *modifies*, found with the
   same detection as `verify` (`diff::tests_in_text`, attribute-marked tests,
   test blocks). Each is identified by name and file.
3. **The proof.** Run each test twice, with the usual selector, runner-output
   rules (failure summaries, zero tests, all skipped) and the scrubbed
   environment:
   * **without the change:** `onto` with the PR's test files overlaid;
   * **with the change:** `head`.

   | without | with | per-test status |
   |---|---|---|
   | fails (assertion) | passes | `proven` |
   | fails (missing name, import or compile error) | passes | `proven_api`: it proves only that the code it calls now exists. That is fine for new features and weak for bug fixes. |
   | passes | passes | `passes_without_change`: it does not test this change |
   | — | fails | `fails_on_pr` |
   | not run | not run | `not_run` (no selector or test command) |

4. **Tampering with existing tests.**
   * An existing test that gains a skip/ignore/xfail marker → **rejected**.
   * Existing tests deleted, whose name exists nowhere in the head → the
     result is **unproven** (needs review). An agent deleting the test that
     blocked it is a classic failure.
5. **Verdict and exit code.**

   | verdict | when | exit |
   |---|---|---|
   | `rejected` | a PR test fails on the PR, or an existing test was skipped | 2 |
   | `neutral` | no code file changed (docs-only or test-only PR) | 0 |
   | `unproven` | code changed but no test is `proven`/`proven_api`; or existing tests were deleted; or `--require-all` and some test `passes_without_change` | 3 |
   | `proven` | otherwise: at least one test fails without the change and passes with it | 0 |

   By default one proving test is enough. A PR can legitimately add
   characterization tests that pass before; those are listed, not failed.
   `--require-all` makes every added or modified test prove the change.
6. **Read-only and base-configured.**
   * It writes no notes.
   * Config (the test command and selector) is read from the **base**, as in
     `verify-pr`, so a PR cannot choose how it is proved.
   * The JSON has `schema: gitent/prove-pr@1`. The Markdown report uses its
     own sticky marker, `<!-- gitent-prove-pr -->`, so it can sit next to a
     `verify-pr` comment.
7. **Shipping.**
   * It is a Gitent subcommand and MCP/HTTP tool (`gitent_prove_pr`).
   * It is a second GitHub Action in the same repository,
     `uses: eladb3/gitent/prove@v0`, sharing the install, run and comment
     scripts, the checksum-verified binaries and the hardening of ADRs
     0014/0016.
   * The GitHub Marketplace lists one action per repository, so a separate
     listing later means a thin wrapper repository; the binary does not
     change.

## Not in this version

* **Hunk-level proof.** This is the next step: revert each code hunk on its own
  and report the hunks that no test needs ("this branch of the fix is
  untested"). It uses the same machinery, run once per hunk.
* **Existing tests that already cover the change.** A PR that fixes a bug
  covered by an already-failing test proves nothing new here; `gitent check`
  and CI cover that case.
* **Mutation testing of the PR's code.** Hunk revert is the cheap version of
  it.
