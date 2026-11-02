# 0010: Target-preservation guards

**Status:** accepted. **Amends:** 0006 (validation policy).

## Context

Fault injection (`python3 evals/run.py --agent stale`) uses a deterministic
"agent" that makes a classic mistake on diverged code: it restores the original
change's files verbatim, then claims success. On the 8 recovery scenarios:

* the baseline (agent alone, trusting its own report): **8 false successes**;
* Gitent with only test-based checks: **4 false successes**.

The 4 that got through either resurrected a module the target had deleted (the
original tests then passed against the resurrected file) or overwrote target
code (`void_order`/`ship_order` disappeared, and no target test covered them).
Tests and requirements only see what they test.

## Decision

Add a deterministic validation check, `target_preserved` (`src/preserve.rs`).
It fails when either of these is true:

1. **Resurrection.** The result contains a path that existed at the fork point
   but which the target deleted, unless the change being moved itself adds it.
   * For a revert, the fork point is "before the reverted change". Paths that
     the reverted change deleted may legitimately come back.
2. **Lost definitions.** A function, class or type defined on the target is
   missing from the result, unless the change being moved itself removes it.
   * Merges protect definitions from both sides.
   * Reverts may remove the definitions the reverted change introduced.
   * Matching is by name, so moving a definition to another file is allowed.

## Consequences

* Fault-injection result after the change: **0 false successes** for Gitent
  (8 for the baseline). Correct reference solutions still pass 9/9, and the live
  Claude Code run must keep passing (`evals/RESULTS.md`).
* The guards are heuristics. They are language-agnostic, use regex definitions,
  and match by name. A legitimate change that deliberately deletes or renames a
  target definition must do so explicitly, as part of the change being moved.
  That is the right default for "move this intention without collateral damage".
