# Vision

**Gitent is the safety and audit layer for agent-driven changes to diverged code**:
backports, fork syncs, merges and reverts.

Coding agents are now good at moving a change onto code that has drifted. In
our evals Claude Code alone got 24/24 real Django backports right, 19 of them
cases where `git cherry-pick` conflicts. What they are not is *accountable*.

* With a faulty agent, trusting the agent's own report let 9 broken real
  backports through. Gitent let through none.
* Nothing records *why* a change exists, what it must keep doing, or how a
  given branch came to contain it.

Gitent provides that.

1. **Verify.** `gitent verify` checks any result, from any agent, bot or human,
   before it lands. It checks the change's requirements and the target's own
   requirements and tests. It also checks nothing was resurrected, deleted or
   weakened. No agent call is needed. (ADR 0011)
2. **Record.** Commits carry their intention and checkable requirements, with
   proof that each check fails before the change and passes after it. A ledger
   of what the code base has promised travels with the repository. (ADR 0008)
3. **Operate**, optionally. `gitent cherry-pick / merge / rebase / revert`
   tries Git first and uses Git's result whenever it validates (no agent at
   all). Otherwise it drives any coding agent in an isolated worktree, with the
   same validation, and only then moves the branch.

What Gitent does **not** claim: making a capable agent more correct. We
measured it, and the result was a tie (`evals/RESULTS.md`, criterion F4).

## Positioning: a Git companion, not a Git replacement

Gitent does not replace Git. Storage, branching, transport, hosting and review
stay Git's job. A Gitent repository *is* a Git repository, and Gitent works on
commits made with plain Git.

| Job | Today | With Gitent |
|---|---|---|
| **Backport a fix** to release branches (by an agent, a bot or a person) | review the diff and hope | `gitent verify <fix>` in CI on each backport PR; or `gitent cherry-pick <fix>` to produce it |
| **Accept agent output** before committing | the agent says "done" | `gitent verify <source> --working-tree` |
| **Keep a fork in sync** with a refactoring upstream | patches rot on every rebase | `gitent rebase upstream/main`, every step validated |
| **Merge parallel agent branches** | conflict markers, resolved by guesswork | `gitent merge`: both sides' requirements must hold |
| **Remove a feature after refactors** | `git revert` conflicts | `gitent revert`: the behaviour must be gone and later work intact |
| **Know why code exists and what must stay true** | commit messages, maybe | `gitent show`, `gitent check`, `gitent log` |

## Commits as contracts

A semantic commit has three parts:

1. **Intention**: what changed and why (title, intent, behaviour, affected symbols).
2. **Requirements**: checkable statements the change must satisfy, preferably
   bound to the project's own tests (see `docs/REQUIREMENTS.md`). Every test a
   commit adds becomes a requirement automatically. Gitent proves at commit time
   that each check fails before the change and passes after it.
3. **Realisation**: the exact Git snapshot, plus provenance.

Requirements accumulate into a **ledger**: everything the code base has
promised and not explicitly retired. Moving a change means making *its*
requirements hold on the target without breaking the target's ledger. That is a
precise, checkable goal, rather than "apply this patch" or "re-implement this
idea".

## Who it is for

1. **Coding agents** (Claude Code, Codex, Kiro, Gemini CLI, aider, custom
   agents) produce many small changes and increasingly need to move them
   between branches, forks and copies. Gitent gives them a machine-readable
   history and a definition of done for every transplant.
2. **Humans supervising agents.** Every operation leaves an auditable trail:
   which intention, which requirements, which source snapshot, which agent,
   which evidence, which result commit.
3. **Automation.** Backport bots and fork-sync bots that produce *validated*
   branches or pull requests instead of conflict reports.

## Principles

1. **Semantic changes are first-class objects**, not AI-generated commit
   messages. They have a schema, an identity, lineage, requirements, and links
   to exact Git snapshots.
2. **Checkable over plausible.** Prefer executable requirements to prose. Judged
   (LLM-evaluated) requirements are allowed but always labelled weaker.
3. **Deterministic first.** Use Git wherever it works. Invoke an LLM only when
   reasoning or code transformation is required, and validate the deterministic
   result too: a clean textual merge can still be wrong.
4. **Bounded agents.** Agents work in isolated worktrees, with explicit tools,
   time limits, attempt limits and explicit validation.
5. **Reproducible provenance.** Every successful operation produces a normal
   Git commit and a record linking intent → requirements → source snapshot →
   result snapshot → evidence.
6. **Never silently overwrite** user changes, and never assume LLM output is
   correct. Uncertain results are rejected or flagged, never merged quietly.
7. **Agent-agnostic.** LLM providers and coding agents are plug-ins.
8. **Git-compatible and removable.** Removing Gitent leaves a valid Git
   history. The records are just notes.

## Non-goals (for now)

* Replacing Git's storage, transport, branching or review.
* Being a test framework. Requirements point at the project's existing tests
  and commands.
* Language-specific semantic analysis (ASTs per language). The MVP uses
  language-agnostic heuristics plus LLM analysis; AST plug-ins are an extension
  point.
* Multi-user servers. Records travel with `git push/fetch` of `refs/notes/gitent`.
