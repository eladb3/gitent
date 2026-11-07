# 0013: Verification integrity: what a green verdict must mean

**Status:** accepted. **Amends:** 0006, 0008, 0010, 0011, 0012. **Prompted by:** the
external hands-on evaluation of 2026-11-07 (correctness findings 1–9).

## Context

An independent reviewer built 15 hand-made backports of one fix and ran
`gitent verify` on each, without an LLM. Nine bad ports were **verified**:

| port | what it did | why v0.1 let it through |
|---|---|---|
| B | no fix; the ported test's asserts were weakened | requirement tests were compared by *name* only |
| E | no fix; the requirement test got `@unittest.skip` | same; a skipped test "passes" |
| K | no fix; the test's expected values were changed to match the bug | same |
| L | proper fix; a target-only test was weakened | `existing_tests_preserved` checks names only |
| N | does nothing; an unrelated `test_total` already existed on the target | test identity was a bare, repo-global name |
| T | unrelated change; the fix had no tests | no executable requirement still meant "verified" |
| J | copied the source branch's whole file (dead code) plus its test | the resurrection guard only covers paths the *target* deleted |
| rev-fake | a "revert" that only deleted the test | a missing test counted as "behaviour gone" |
| F | hard-coded the two tested inputs | a test oracle cannot see this; only review can |

Other weaknesses showed up as well:

* A selector that matched **zero tests** exited 0 (unittest before Python 3.12,
  `cargo test <filter>`, `go test -run`, `jest -t`), so a requirement passed
  without running.
* The "before" test list came from git objects and the "after" list from a
  filesystem walk that skipped `build/`, `dist/` and `target/`. Committed tests
  in those directories were therefore reported as removed.
* Ledger retirements ignored order, and merges could bring back requirements
  that one side had retired.

The README promised "no tests deleted or weakened … no AI needed". Only
deletion was checked.

## Decision

### 1. Three verdicts, not two

| verdict | meaning | `verify` / `verify-pr` exit |
|---|---|---|
| `verified` | every check passed **and** at least one executable requirement of the source (or, for a revert, of the reverted change) was executed and proved the move | 0 |
| `rejected` | a check found the move wrong | 2 |
| `unverified` | nothing definitive is wrong, but the move is **not proven**: nothing executable exists, or a test was changed in a way only a human can judge | 3 |

A check can fail in *review* mode: `passed = false, review = true`. The verdict
is `unverified` when every failing check is a review failure, and `rejected`
when any failure is definitive.

Inside operations (cherry-pick, merge, rebase, revert), a review failure does
not block. It becomes a `needs_review: …` warning on the operation and its
record. The test suite found the reason: a target that counts money in cents
legitimately changes a ported test's expected value from `113` to `11300`.
Definitive failures still block operations.

`verify` JSON gains `"schema": "gitent/verify@1"` and `"verdict"`.
`verify-pr` results gain `unverified`, and its JSON gains
`"schema": "gitent/verify-pr@1"`.

### 2. Test integrity (`tests_intact`)

For each requirement test of the source, compare the test's block in the
result (decorators/attributes plus body) with the block in the source commit.
For each test that existed at the base and that the move modified, compare the
base block with the result block. The comparison:

* **definitive** (`rejected`): a skip, ignore or xfail marker was added
  (`@unittest.skip`, `@pytest.mark.skip/xfail`, `self.skipTest`, `pytest.skip`,
  `#[ignore]`, `it.skip`/`xit`/`test.todo`, `t.Skip`, `@Disabled`/`@Ignore`), or
  every assertion was removed;
* **review** (`unverified`): fewer assertions, or different literals on
  assertion lines.

Renaming identifiers does not change literals, so a port that adapts a test to
a renamed API stays green.

### 3. Qualified test identity

Requirements of kind `test` keep `test` (the name) and gain `test_path` (the
file it was added in). Records are bumped to `gitent/semantic-change@3`;
records using `@2` read unchanged. A requirement test is *located* in a result
as follows:

1. If the file at `test_path` defines it, use that file.
2. Otherwise, use a file where the move **added** that definition. A port may
   move a test to another file.
3. Otherwise the test is **not ported**. A same-named test that already
   existed at the base, somewhere else, does not count.

Selectors may use `{file}` and `{module}` (dotted path without the extension)
next to `{test}`. Each is shell-quoted. Detected defaults run exactly one test
where the runner allows it (`pytest {file} -k {test}`,
`python -m unittest {module}` + `-k`).

### 4. Zero tests ran is a failure

A requirement run passes only if the command succeeded **and** its output does
not show that zero tests ran. The patterns covered: `Ran 0 tests`,
`no tests ran`, `running 0 tests` in every section, `no tests to run`,
`0 passing`, `No tests found`, and jest's `Tests: N skipped, N total`. At
commit time, this makes a vacuous requirement fail its proof instead of passing
with a warning.

### 5. Revert evidence

`revert_effective` overlays the reverted change's test files onto a copy of the
result and requires that the reverted requirements **fail**. A requirement
whose test still cannot be found after the overlay is a review failure. A
revert of a change with no executable requirements is `unverified`.

### 6. Orphan copies (`no_orphan_copy`, review)

If the move *creates* a file that the source commit only *modified*, and that
file does not exist at the base, the port probably copied the source branch's
code instead of adapting the target's. This is a review failure. Port J is the
example.

### 7. verify-pr verifies the PR, not just the marked commits

* Every detected source is verified with `result = head`, `onto = merge-base(base, head)`.
  A later commit in the PR that breaks the port is therefore seen.
* Every marker in every commit message counts, including squashed messages
  with several markers.
* A source commit inside `base..head`, meaning the PR citing itself, is
  refused.
* `verify-pr` never writes notes. Heuristic records are computed in memory.

### 8. Consistent test lists

The "before" and "after" test inventories both come from git trees: the base
tree and the result (or snapshot) tree.

### 9. Ledger order

`ledger(tip)` walks newest first and keeps the newest event per requirement:
introduced → live, retired → gone. Revert-then-reapply therefore works. For a
merge, a requirement retired on one side since the merge base stays retired,
unless the other side introduced it again since the base.

### 10. Failures are not swallowed

* An unparsable note is an error on `put`; it is never silently overwritten.
* A `target_preserved` check that cannot run is a review failure, not a
  warning.

### 11. Language coverage

Tests are also detected when they are marked by an attribute or annotation on
a preceding line: `#[test]`, `#[tokio::test]`, `@Test`, `[Test]`, `[Fact]`,
`[TestMethod]`. Swift `func test…` and RSpec `it "…" do` are detected as well.
Definitions now include Java/C#/Kotlin/Swift methods and typed TypeScript arrow
functions.

### 12. Stubbed bodies (`bodies_intact`, review)

`target_preserved` compares definition *names*, so emptying a function's body
passed. A definition that has a real body at the base and is reduced to a stub
in the result (`pass`, `...`, `todo!()`, `{}`) is a review failure, unless the
moved change itself does the same.

## What remains out of reach

* **Port F.** Hard-coding the tested inputs passes any test oracle. Only a
  reviewer, human or LLM verifier, can see it. The Action can opt into the LLM
  verifier (ADR 0014).
* Assertion and literal comparison is a heuristic. A legitimate port that
  changes expected values (for example, units differ on the target) is
  `unverified`, and a human should look at it.
