# Instructions for agents working on Gitent

Gitent is built by agents, for agents. Read this before changing anything.

## Read first (in order)

1. `docs/VISION.md` — what and why.
2. `docs/SUCCESS_CRITERIA.md` — the contract. Every change should move a
   criterion forward or keep them all green.
3. `docs/ARCHITECTURE.md` — module map, data model, engine flow.
4. `docs/adr/` — decisions already made. Do not silently reverse one; add a new
   ADR that supersedes it.

## Workflow

1. Pick (or add) a criterion in `docs/SUCCESS_CRITERIA.md`.
2. Write/extend the integration test that proves it (`tests/`), watch it fail.
3. Implement. Keep the core deterministic; LLM calls only behind
   `llm::Provider` / `agent::Executor`.
4. `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test`.
5. Update the criterion's "Verified by" column and any affected docs.

## Rules

* Never let an operation touch the user's working tree before validation
  succeeded; never overwrite uncommitted changes.
* Never trust LLM output: everything it claims must be checked against the diff
  or validated by tests.
* No vendor coupling in core modules. New providers/executors are new files in
  `src/llm/` or `src/agent/`.
* Every public JSON shape is part of the API: changing it requires bumping
  `schema` versions and updating `gitent schema`.
* Integration tests must run offline and deterministically. Use `command`
  executors (shell scripts in `tests/fixtures/agents/`) and `scripted`/`command`
  providers as stand-ins. Real-LLM checks go in `evals/`.

## Layout

```
src/main.rs            CLI
src/api.rs             tool surface shared by CLI --json, MCP and HTTP
src/mcp.rs, http.rs    servers
src/ops/               engine + operations
src/analyze.rs         semantic extraction
src/validate.rs        validation checks
src/store.rs           notes + op journal
src/repo.rs            git access
src/llm/, src/agent/   providers and executors
tests/                 integration tests (real git, real processes)
evals/                 comparative evals with real agents
integrations/          drop-in files for Claude Code, Codex, Kiro
```
