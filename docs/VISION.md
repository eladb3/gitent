# Vision

Git records **what the code is**. Gitent also records **what each change is
for** and **what it must keep doing**. It uses those records to move changes
safely between versions of a code base that have drifted apart.

## Positioning: a Git companion, not a Git replacement

Gitent does not replace Git. Storage, branching, transport, hosting and review
stay Git's job. A Gitent repository *is* a Git repository, and Gitent works on
commits made with plain Git.

Gitent is for the moments where Git's model breaks: a change must land on code
that has **diverged** from where it was written, because functions were
renamed, files moved, signatures changed or units changed. There, a textual
patch conflicts or, worse, applies cleanly and is wrong. The change's *purpose*
and its *requirements* are still perfectly applicable.

Day to day you keep using Git. You reach for Gitent when moving work around:

| Job | Today | With Gitent |
|---|---|---|
| **Backport a fix** to release branches that were refactored differently | manual re-implementation per branch | `gitent cherry-pick <fix>` on each branch; the fix's requirements prove it landed |
| **Keep a fork or vendored copy in sync** with an upstream that refactors | patches rot on every rebase | `gitent rebase upstream/main` replays intentions, not patches |
| **Merge many parallel agent branches** touching the same area | conflict markers, resolved by guesswork | `gitent merge` composes intentions; both sides' requirements must hold |
| **Remove a feature after later refactors** | `git revert` conflicts | `gitent revert` removes the behaviour; the feature's requirements are retired |
| **Know why code exists and what must stay true** | commit messages, maybe | `gitent show` / `gitent check`: intentions and live requirements, machine-readable |

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
