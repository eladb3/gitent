# 0009: Operation worktrees live outside `.git/`

**Status:** accepted. **Amends:** 0001, ARCHITECTURE storage layout.

## Context

Worktrees were created at `.git/gitent/worktrees/<op_id>`. The offline test
suite passed, because shell-script stand-in agents do not care where they run.
The first live eval (`evals/RESULTS.md`, run 1) used Claude Code as the
executor, and Claude Code **refused to edit any file under a `.git/` path**.
Most agent attempts produced no changes, and Gitent scored 3/6 against 6/6 for
the same agent used without Gitent. The agent's own transcript named the
cause: "have Gitent create its worktrees outside `.git`".

## Decision

* Worktrees go under `executor.worktree_root`, defaulting to
  `<system tmp>/gitent-worktrees/<repo-name>-<hash of git dir>/<op_id>`.
* Temporary proof and check worktrees use the same root.
* Briefs and the op journal stay in `.git/gitent/ops/` because agents only read them.
* `tests/cherry_pick.rs::host_mode_roundtrip` asserts the worktree is outside the repository.

## Consequences

* Worktrees in the system temp directory can be cleaned by the OS. An
  interrupted operation can then no longer be continued, and must be aborted
  and restarted. The branch is never affected.
* Lesson: stand-in agents cannot catch integration constraints of real agents.
  The live eval must run before claims are made (see the plan in
  `docs/process/AUTONOMOUS_DEVELOPMENT.md`).
