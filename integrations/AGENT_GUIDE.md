# Using Gitent from a coding agent

Gitent is a Git companion: keep using git for status, diff, branches and push.
Use Gitent when you **commit** (so your intention and requirements are recorded)
and when you **move work between diverged branches**.

## Committing
- Call `gitent_commit` (or `gitent commit -m ...`) instead of `git commit`.
- Pass your own `intent` — you know why you made the change:
  `{"title": "...", "intent": "why", "behavior": "observable result"}`.
- Pass `requirements` for anything that must hold, bound to a test when possible:
  `[{"statement": "repeat lookups hit the db once", "test": "test_cache_hit"}]`.
  Every test you add becomes a requirement automatically.
- If the commit is rejected, a requirement fails on your change or does not
  discriminate it (passes even without your change). Fix the code or the test,
  do not delete tests.
- To deliberately change promised behaviour, retire the requirement with a reason:
  `"retires": [{"id": "req_…", "reason": "…"}]`.

## Moving work (cherry-pick, merge, rebase, revert)
- Call `gitent_cherry_pick` / `gitent_merge` / `gitent_rebase` / `gitent_revert`.
- If plain git works and validates, it is done for you.
- If the result is `"state": "awaiting_host"`:
  1. Read the brief at `brief` — its **Definition of done** lists the requirements Gitent will check.
  2. Edit files **only inside `worktree`**. Do not commit there.
  3. Call `gitent_continue`. On failure you get feedback and can try again; `gitent_abort` gives up.
- Your branch is only updated after validation passes.

## Verifying your own work (do this before you say "done")
- After you backport, merge or revert anything by editing files yourself:
  `gitent_verify {"source": "<commit you moved>", "working_tree": true}` (`--kind revert|merge` as needed).
  It checks the change's requirements, the target's requirements and tests, and that you did not
  resurrect deleted files, drop existing definitions or delete tests. Fix what it reports.
- For a commit or PR branch: `gitent_verify {"source": "...", "result": "HEAD", "onto": "<target branch before your work>"}`.
- Add `"record": true` on a verified commit to attach provenance and requirements to it.

## Checking
- `gitent_check` runs every live requirement and names the intention behind any failure.
- `gitent_log` / `gitent_show` explain why code exists and what it must keep doing.
