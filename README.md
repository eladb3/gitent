<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/wordmark-dark.svg">
  <img src="docs/assets/wordmark-light.svg" alt="gitent" width="300">
</picture>

### Did that backport *actually* carry the fix?

**Gitent checks it, for backports, cherry-picks, merges and reverts made by people, bots or AI agents.**

[![ci](https://github.com/eladb3/gitent/actions/workflows/ci.yml/badge.svg)](https://github.com/eladb3/gitent/actions/workflows/ci.yml)
[![action self-test](https://github.com/eladb3/gitent/actions/workflows/action-selftest.yml/badge.svg)](https://github.com/eladb3/gitent/actions/workflows/action-selftest.yml)
![status](https://img.shields.io/badge/status-experimental-orange)
![works with](https://img.shields.io/badge/works%20with-Claude%20Code%20%C2%B7%20Codex%20%C2%B7%20Kiro%20%C2%B7%20any%20agent-blueviolet)

<img src="docs/demo/demo.gif" alt="gitent demo: git cherry-pick conflicts, an agent's quick port is rejected, a proper port is verified" width="860">

<sub>60-second demo (real output, reproducible with <a href="docs/demo/demo.sh"><code>docs/demo/demo.sh</code></a>) · <a href="docs/demo/demo.mp4">MP4</a></sub>

</div>

---

## The problem

You fix a bug on one branch and need the same fix on another, a release branch
or a fork. But **the code there has moved on**: functions were renamed, files
moved, units changed.

```mermaid
%%{init: {'gitGraph': {'rotateCommitLabel': false, 'showBranches': true}} }%%
gitGraph
  commit id: "v1.0"
  branch 1.x
  commit id: "the fix" type: HIGHLIGHT
  checkout main
  commit id: "refactor"
  commit id: "port?" type: REVERSE
```

* `git cherry-pick` **conflicts**, or worse, applies cleanly and is **silently wrong**.
* So a person or an AI agent re-does the fix by hand, and says *"done, tests pass"*.
* Do the tests prove the fix actually landed? Not necessarily. A quick port can
  copy the old file back, so the tests pass against the resurrected code while
  `main`'s real code never gets the fix. Or a test gets deleted along the way.

## What Gitent does

Gitent records **what each change is for and what it must keep doing**. It then
**checks** that every port of that change still does it.

| | | |
|---|---|---|
| 🧾 **Record** | `gitent commit` | Stores the change's intent and **requirements** next to the Git commit. Every test you add becomes a requirement, *proven* to fail before the change and pass after it. |
| 🧪 **Prove** | `gitent prove-pr main` | For **any** PR: every test it adds or changes must **fail without the change** and pass with it. A test that passes either way does not test the change, however it is written. |
| 🔍 **Verify** | `gitent verify <fix>` | Checks that a port carries the fix: the fix's requirements pass on the target, the target's own tests and requirements still pass, nothing was resurrected, dropped or deleted, and no test was skipped or weakened. Deterministic: **no AI needed**. An LLM verifier can be added. |
| 🤖 **Operate** *(optional)* | `gitent cherry-pick <fix>` | Tries Git first. If Git can't do it, hands the job to *your* coding agent in an isolated worktree, verifies the result, and only then moves your branch. |

It is **not a Git replacement.** A Gitent repo is a Git repo, and records live
in Git notes (`refs/notes/gitent`). Keep using Git every day; reach for Gitent
when a change has to move.

## What `verify` checks

```mermaid
flowchart LR
  P["a port of the fix<br/>(by a person, bot or agent)"] --> V{gitent verify}
  V --> R1["the fix's requirements<br/>pass on the target"]
  V --> R2["the target's tests and<br/>requirements still pass"]
  V --> R3["no deleted files<br/>brought back"]
  V --> R4["no existing functions<br/>dropped"]
  V --> R5["no tests deleted,<br/>skipped or weakened"]
  R1 & R2 & R3 & R4 & R5 --> OK(["✓ verified · exit 0"])
  V -. something is wrong .-> NO(["✗ rejected · exit 2"])
  V -. nothing proves it .-> RV(["? unverified · exit 3<br/>a human should look"])
```

The output on a bad port, from the demo above:

```text
$ gitent verify 1.x
✗ rejected: apply of 6ff94c5b2c onto 18d473b5b8 → df7bda7eb6
  ✓ requirements               1 executable requirement(s) satisfied
  ✓ tests                      `python3 -m unittest discover -q` passed in 47 ms
  ✗ target_preserved           recreates path(s) the target deliberately deleted: app/users.py
```

The tests pass, the agent says "done", and Gitent still says no.

**Three verdicts.**
* `verified`: something executable proved the move.
* `rejected`: something is definitely wrong, such as a deleted or skipped test,
  a failing requirement, a resurrected file, or a ported test that **also passes
  on the unfixed target**. Gitent proves the ported test can fail, the same way
  `gitent commit` proves it on the source.
* `unverified`: nothing is known to be wrong, but nothing proves the move
  either. The fix had no tests (or none ran), a target-only test was
  weakened, a test-harness file changed, or code was copied from the source
  branch. A human should look.

**What it cannot see.** A port that hard-codes exactly the tested inputs
passes any test-based check. Only review catches that, by a person or the
optional LLM verifier ([what is checked and what isn't](docs/adr/0013-verification-integrity.md)).

## Quick start

```bash
cargo install --git https://github.com/eladb3/gitent      # prebuilt binaries arrive with the first release
cd your-repo && gitent init                              # detects your test command
```

**1. Record a fix** (the test you add becomes a proven requirement):

```bash
gitent commit -m "fix: cache user lookups"
#   require  req_9e22… cache hit (test test_cache_hit)  [before: fail, after: pass]
```

**2. Port it** however you like (by hand, with `git cherry-pick`, or with an
agent), **then verify:**

```bash
gitent verify <fix>                 # the commit you just made on this branch
gitent verify <fix> --working-tree  # uncommitted changes (e.g. right after an agent edited files)
gitent verify <fix> --onto main     # a whole multi-commit branch against its target
gitent verify <fix> --kind revert   # a revert: is the behaviour really gone?
gitent verify <fix> --record        # if verified, record the port's provenance
```

**3. Check what the code base has promised:**

```bash
gitent check        # run every live requirement; names the change behind any failure
gitent log          # history as intentions, with where each port came from
```

## Prove every PR's tests

AI agents (and people) write tests that cannot fail: asserts on the mock,
expected values copied from the buggy output, tests of something next to the
change. `prove-pr` runs each test the PR adds or modifies twice: on the base
with the PR's tests but **without its code**, then on the PR.

```text
$ gitent prove-pr main
  ✓ tests/test_discount.py::test_discount_clamped  without: fail  with: pass  proves the change
  ? tests/test_discount.py::test_zero_discount     without: pass  with: pass  passes WITHOUT the change: does not test it
✓ proven: 1 of 2 test(s) fail without the change and pass with it
```

| result | when | exit |
|---|---|:---:|
| ✅ `proven` | at least one new/changed test fails without the change (`--require-all`: every one) | 0 |
| ⚠️ `unproven` | code changed, but no test fails without it; or existing tests were deleted | 3 |
| ❌ `rejected` | a PR test fails on the PR, or an existing test was skipped | 2 |
| ➖ `neutral` | no code changed | 0 |

No notes, markers or `gitent commit` needed. Agents can run it on their own
uncommitted work (`--working-tree`, or the MCP tool `gitent_prove_pr`) before
saying "done". In CI:

```yaml
      - uses: actions/checkout@v4
        with: { fetch-depth: 0, persist-credentials: false }
      - run: pip install -e . pytest
      - uses: eladb3/gitent/prove@v0
        with:
          test-command: "python -m pytest -q"
          test-selector: "python -m pytest -q {file} -k {test}"
```

## Use it in CI: verify every backport PR

```yaml
# .github/workflows/verify-backports.yml
on:
  pull_request:
    branches: ["stable/**", "release/**"]
permissions: { contents: read, pull-requests: write }
jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - run: pip install -e . pytest          # your project's usual test setup
      - uses: eladb3/gitent@v0
        with:
          test-command: "python -m pytest -q"
          test-selector: "python -m pytest -q {file} -k {test}"
```

The action:
* finds the original commit from the markers backports already carry:
  `(cherry picked from commit …)` (`git cherry-pick -x`, most backport bots),
  `Backport of …`, `This reverts commit …`;
* posts **one** PR comment and keeps it up to date;
* **fails the check** on a rejected backport, and on an unverified one unless
  you set `allow-unverified: true`;
* runs deterministically (`--no-llm`) unless you opt into an LLM verifier with
  `llm-provider`;
* reads the test command and config from the PR's **base**, so a PR cannot
  choose how it is checked.

Full reference: [docs/GITHUB_ACTION.md](docs/GITHUB_ACTION.md). Ready-made
workflows are in [examples/](examples/).

## Use it with AI coding agents

<table>
<tr><th>Your agent drives Gitent</th><th>Gitent drives your agent</th></tr>
<tr><td>

```bash
claude mcp add gitent -- gitent mcp
```

The agent calls `gitent_verify` on its own work before saying "done". It calls
`gitent_commit` with its intent, and `gitent_cherry_pick` in *host mode* (it
edits, Gitent validates). Setups for Claude Code, Codex and Kiro are in
[integrations/](integrations/).

</td><td>

```bash
gitent init --agent claude-code   # or codex, kiro, gemini, aider
gitent cherry-pick <fix>
```

Gitent tries Git first. Otherwise it runs your agent in an isolated worktree,
verifies the result, retries with feedback, and moves the branch only when the
result is verified.

</td></tr>
</table>

Everything is also available as JSON (`--json`), over MCP (`gitent mcp`) and
over HTTP (`gitent serve`, token-protected). The tool surfaces cannot choose
which commands Gitent runs; that comes only from config and the CLI
([trust boundaries](docs/adr/0014-trust-boundaries.md)).

**Core vs experimental.** `verify`, `verify-pr`, the Action, `commit` and
`check` are the core. The operations (`cherry-pick`, `merge`, `rebase`,
`revert`), `diff`, `annotate` and `serve` are marked `[experimental]` in
`--help`.

## Does it work? The evidence

Measured on **24 real Django backports**. In 19 of them `git cherry-pick`
conflicts. The hidden oracle is the *human* backport's tests
([evals/RESULTS.md](evals/RESULTS.md)).

| | plain git | Claude Code alone | Claude Code + Gitent |
|---|:---:|:---:|:---:|
| backports correct | 5 / 24 | **24 / 24** | **24 / 24** |
| **wrong results reported as success**, with a faulty "stale port" agent | — | ❌ 9 | ✅ **0** |
| …same, with `gitent verify` as a gate behind the agent | — | ✅ **0** | |
| extra cost of the gate | | ~18 s per backport, no AI tokens | |

Hand-made bad backports from an external review, checked by `gitent verify`
with no LLM ([tests/fault_models.rs](tests/fault_models.rs)):

| fault | v0.1 | now |
|---|:---:|:---:|
| test skipped | ❌ verified | ✅ rejected |
| asserts weakened, made always-true, short-circuited, errors swallowed · expectations rewritten to match the bug · harness forcing exit 0 | ❌ verified | ✅ rejected: the ported test passes on the unfixed target, so it proves nothing |
| a same-named old test stands in for the ported one | ❌ verified | ✅ rejected |
| "revert" that only deletes the test | ❌ verified | ✅ rejected |
| target's own test weakened · the fix had no tests · source branch's file copied over | ❌ verified | ⚠️ unverified |
| proper port with the test adapted to the target's API | ✅ verified | ✅ verified |
| exactly the tested inputs hard-coded | ❌ verified | ❌ verified (needs a reviewer) |

**Honest summary:**
* Gitent doesn't make a good agent smarter; on correctness it ties.
* The "0 false successes" figure is for one fault model, the stale port. The
  table above adds the ones an external reviewer found. Each is now a
  regression test.
* It only catches what is recorded or tested. Behaviour that matters only on
  the target branch, and that nothing there tests, can slip through. We saw
  this once. Hard-coded test inputs also need a reviewer.

## How it works

```mermaid
flowchart TB
  IN["CLI · MCP server · HTTP API · GitHub Action"]:::d --> CORE
  subgraph CORE ["gitent core (Rust)"]
    direction LR
    REC["records + requirements<br/>refs/notes/gitent"]
    VAL["validation<br/>requirements · tests · guards"]
    ENG["operation engine<br/>git first → agent → validate → move branch"]
  end
  CORE --> GIT[("your Git repo")]
  ENG -. optional .-> AG["any coding agent<br/>Claude Code · Codex · Kiro · …"]
  classDef d fill:#eef,stroke:#88a
```

* **Records** are JSON in Git notes, so they push and fetch with the repo.
  Commits also carry a `Gitent-Intent:` trailer.
* **Operations** run in an isolated worktree. Your working tree is only touched
  after validation passes, and never when it has uncommitted changes.
* Design docs: [Vision](docs/VISION.md) · [Architecture](docs/ARCHITECTURE.md) ·
  [Requirements spec](docs/REQUIREMENTS.md) · [Decision records](docs/adr/) ·
  [Success criteria](docs/SUCCESS_CRITERIA.md).

## Status and contributing

**Experimental (v0.1).** CI runs 90+ tests (unit, plus integration tests on real Git repositories).
The action is tested on GitHub's own runners. Known limits are documented in
[evals/RESULTS.md](evals/RESULTS.md).

Gitent is built *by* agents, *for* agents. Contributors, human or AI, start at
[AGENTS.md](AGENTS.md).

```bash
cargo build --release && cargo test        # requires git ≥ 2.30 and python3 for the test fixtures
```
