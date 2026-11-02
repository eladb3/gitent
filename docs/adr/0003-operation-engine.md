# 0003 — One step-based operation engine

**Status:** accepted

**Decision.** cherry-pick, revert, rebase and merge are all expressed as an
ordered list of steps (`apply`, `revert`, `merge`) run by one engine:

`deterministic attempt → validate → (agent attempt → validate)* → commit-in-worktree`,

then a compare-and-swap branch update and a safe checkout.

* All work happens in an isolated worktree under `.git/gitent/worktrees/`; the
  user's working tree is untouched until finalize.
* Start is refused if the working tree is dirty (principle 5).
* Finalize fails if the branch moved since the operation started.
* The operation is journaled to `.git/gitent/ops/<id>.json`, which makes host
  mode (pause/continue) and post-mortem inspection possible.

**Why validate the deterministic result.** A conflict-free textual merge can
be semantically wrong; running the tests on it is cheap and is the only way to
honour "never assume correctness" for both Git and LLM output.
