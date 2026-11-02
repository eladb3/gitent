# 0007 — Testing strategy

**Status:** accepted

* **Unit tests** inside modules for parsing, schema, path confinement,
  heuristics.
* **Integration tests** (`tests/*.rs`) run the real `gitent` binary against
  real temporary Git repositories with real Python test suites. The "agent" is
  a real process: a small shell script registered as a `command` executor that
  performs the edit a competent agent would make (or a deliberately bad one).
  The "LLM" for analysis is a `command` or `scripted` provider. This exercises
  every production code path (process spawning, briefs, worktrees, validation,
  notes, CAS finalize) deterministically and offline.
* **Evals** (`evals/`) run the same scenarios with *real* coding agents and
  hidden tests, comparing plain Git, a baseline agent, and Gitent. They are
  costly and non-deterministic, so they are run on demand and their results are
  committed to `evals/RESULTS.md`.
