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
