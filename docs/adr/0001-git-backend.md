# 0001 — Git backend: git2 + git CLI for worktrees

**Status:** accepted

**Context.** Gitent must not reimplement Git. It needs: object/tree/commit
creation, diffs, notes, refs with compare-and-swap, in-memory
cherry-pick/merge/revert to *test* whether a deterministic operation is
conflict-free, and isolated working directories for agents.

**Decision.** Use `git2` (libgit2, vendored, no network features) for all
object, ref, note, diff and in-memory merge operations. Use the `git` CLI only
for `git worktree add/remove/prune`, because libgit2's worktree API requires a
branch per worktree and real coding agents (Claude Code, Codex, …) expect a
normal checkout in which `git diff`/`git status` work.

**Consequences.** `git` ≥ 2.30 must be on `PATH`. Worktrees appear in
`git worktree list` while an operation is in progress; `gitent abort` and
successful completion remove them.
