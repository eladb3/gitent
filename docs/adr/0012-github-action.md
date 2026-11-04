# 0012: GitHub Action, `gitent verify` on backport pull requests

**Status:** accepted. **Builds on:** 0011. **Spec:** `docs/GITHUB_ACTION.md`.

## Context

`gitent verify` is the measured value of Gitent (`evals/RESULTS.md`). With a
faulty agent it cuts false successes from 9 to 0 on real Django backports, with
no agent cost and about 18 s per check. Most backports are made by people or
bots, and reviewed in pull requests. The place to put `verify` is therefore the
pull request's CI.

## Decision

1. **`gitent verify-pr --base <ref> [--head <ref>]`** (in Gitent, so it is
   tested like everything else). It finds which commits of `base..head` are
   moves of an earlier change, verifies them, and writes a JSON result and a
   Markdown report.
   * **Source detection**, in this order:
     1. `--source <rev>`, given explicitly;
     2. commit trailers: `Gitent-Derived-From: <sha>`,
        `(cherry picked from commit <sha>)` (written by `git cherry-pick -x`),
        `Backport of <sha>` (Django and others), `Backported from <sha>`;
     3. otherwise the PR is not a backport, and the result is `neutral`
        (exit 0, unless `--require-source`).
   * **Granularity.**
     * If every detected source belongs to one commit, or there is exactly one
       source, the whole PR (`base..head`) is verified against it, so
       follow-up fix commits count.
     * Otherwise each commit carrying a trailer is verified against its own
       parent.
   * A source that is not present in the clone is a configuration error
     (exit 1), with a hint to use `fetch-depth: 0`.
   * `--kind` is inferred: `revert` for "This reverts commit <sha>." (the
     standard `git revert` message), `apply` otherwise.
2. **A composite action, `action.yml`, at the repository root**, used as
   `uses: eladb3/gitent@v0`. It:
   * installs Gitent from the matching GitHub release (Linux and macOS
     binaries), falling back to `cargo install --git`;
   * fetches `refs/notes/gitent` if the remote has it;
   * runs `verify-pr` and writes the Markdown to `$GITHUB_STEP_SUMMARY`;
   * optionally maintains **one** sticky PR comment, found by a hidden marker
     and edited in place rather than duplicated;
   * exits with Gitent's code, so a failed verification fails the check.
3. **Project setup stays the user's job.** The workflow installs dependencies
   before the action, as for any test job. The action takes `test-command` and
   `test-selector` inputs that override `.gitent/config.toml`.
4. **Gitent's own CI and release.**
   * `ci.yml`: fmt, clippy, tests.
   * `release.yml`: on a `v*` tag, build binaries and attach them to the
     GitHub release.
   * `action-selftest.yml`: runs the action from the checkout (`uses: ./`)
     against generated repositories, a good backport and three bad ones, and
     asserts the outcomes on real GitHub runners.

## Consequences

* Adoption needs no change to how backports are made, and no AI.
* The action is only as good as the project's test command and the recorded
  requirements. The known blind spot is target-only behaviour (RESULTS.md, V8).
* The comment needs `pull-requests: write`. Without it, only the step summary
  and the check status are produced.
