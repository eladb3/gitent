# Architecture Decision Records

Short, numbered, immutable once accepted (supersede instead of editing).

| # | Decision |
|---|----------|
| [0001](0001-git-backend.md) | git2 for objects/refs/notes/in-memory merges; git CLI for worktrees |
| [0002](0002-semantic-store-in-notes.md) | Semantic records live in `refs/notes/gitent` as JSON |
| [0003](0003-operation-engine.md) | One step-based operation engine for all operations; isolated worktrees; CAS finalize |
| [0004](0004-agent-abstraction.md) | Separate *Provider* (text completion) from *Executor* (code-editing agent); three executor kinds |
| [0005](0005-json-action-protocol.md) | Builtin agent uses a provider-agnostic JSON action protocol, not native tool-calling |
| [0006](0006-validation-policy.md) | Deterministic validation gates; LLM verifier is advisory unless configured strict |
| [0007](0007-testing-strategy.md) | Integration tests use real processes as stand-in agents; real-LLM evals are separate |
| [0008](0008-requirements.md) | Requirements are first-class, checkable parts of a commit; ledger; proof at commit time (amends 0006) |
| [0009](0009-worktrees-outside-git-dir.md) | Operation worktrees live outside `.git/` (found by the first live eval) |
| [0010](0010-target-preservation-guards.md) | Results must not resurrect deleted paths or drop target definitions (found by fault injection) |
| [0011](0011-verify.md) | `gitent verify`: validate any agent's/bot's/human's result without driving the agent |
| [0012](0012-github-action.md) | GitHub Action: `gitent verify-pr` on backport PRs; release binaries; self-test on real runners |
| [0013](0013-verification-integrity.md) | Three verdicts; test integrity, qualified test identity, zero-tests, revert evidence (external review, round 1) |
| [0014](0014-trust-boundaries.md) | Trust boundaries: serve token, tool surfaces cannot choose commands, op_id, process groups, containment |
| [0015](0015-proof-on-the-target.md) | Prove the ported test on the target; heuristics advisory; runner summaries read (round 2) |
| [0016](0016-trust-boundaries-round-2.md) | Scrubbed child environments, isolated Action builds, agent commands need consent, `gitent trust`, remote `serve` opt-in (extends 0014) |
| [0017](0017-prove-pr.md) | `gitent prove-pr` and `eladb3/gitent/prove`: every test a PR adds must fail without its change and pass with it |
