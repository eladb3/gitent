# Vision

Git tracks textual changes. **Gitent tracks semantic intentions** and uses LLM
agents to execute version-control operations that require reasoning.

A *semantic commit* describes **what changed and why**, not just which lines
changed. Operations like merge, rebase, cherry-pick and revert reason about
these intentions instead of blindly applying patches.

## Who it is for

1. **Coding agents** (Claude Code, Codex, Kiro, Gemini CLI, aider, custom
   agents). They produce many small, well-described changes and increasingly
   need to move them between branches, forks, and diverged copies of a code
   base. Gitent gives them a machine-readable history and a safe, validated way
   to transplant intent.
2. **Humans supervising agents.** Every operation leaves an auditable trail:
   which intention, which source snapshot, which agent, which validation, which
   result commit.

## Principles

1. **Semantic changes are first-class objects**, not AI-generated commit
   messages. They have a schema, an identity, lineage, and links to exact Git
   snapshots.
2. **Deterministic first.** Use Git's own machinery wherever it works; invoke
   an LLM only when semantic reasoning or code transformation is required —
   and validate the deterministic result too (a clean textual merge can still
   be semantically wrong).
3. **Bounded agents.** Agents work in isolated worktrees, with explicit tools,
   time limits, attempt limits, and explicit validation.
4. **Reproducible provenance.** Every successful operation produces a normal
   Git commit and a semantic record linking intent → source snapshot → result
   snapshot → validation evidence.
5. **Never silently overwrite** user changes and never assume LLM output is
   correct. Uncertain results are rejected or flagged, never merged quietly.
6. **Modular & agent-agnostic.** LLM providers and coding agents are plug-ins.
   The core never depends on a specific model or vendor.
7. **Git-compatible.** A Gitent repository *is* a Git repository. Removing
   Gitent leaves a valid history.

## Non-goals (for now)

* Replacing Git's storage, transport, or object model.
* Language-specific semantic analysis (ASTs per language). The MVP uses
  language-agnostic heuristics plus LLM analysis; AST plug-ins are a future
  extension point.
* Multi-user servers. Semantic notes travel with `git push/fetch` of
  `refs/notes/gitent`.
