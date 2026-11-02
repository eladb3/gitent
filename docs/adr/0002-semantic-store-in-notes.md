# 0002 — Semantic records in `refs/notes/gitent`

**Status:** accepted

**Options.** (a) sidecar files committed in the tree, (b) a custom ref with its
own commit chain, (c) Git notes.

**Decision.** Git notes, one JSON document per realising commit, under
`refs/notes/gitent`.

* Native, content-addressed, versioned (the notes ref is itself a commit chain).
* Do not pollute the source tree or change tree hashes (so a Gitent commit and
  a Git commit with the same content have the same tree).
* Shareable: `git push origin refs/notes/gitent`, `git fetch origin refs/notes/gitent:refs/notes/gitent`.
* Commits also carry a `Gitent-Intent: int_…` trailer so the link survives
  tools that drop notes (e.g. `git rebase` outside Gitent).

**Identity.** `intent_id` identifies the *intention* and is preserved across
realisations (cherry-pick, rebase). A revert or merge creates a new intent that
references others (`reverts`, `composes`). Lookup by `intent_id` scans notes
(O(n)); an index is a future optimisation.
