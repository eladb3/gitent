# GitHub Action: verify backport pull requests

The action checks that a backport, cherry-pick or revert PR correctly carries
the original change onto its target branch:

* the change's own tests and requirements pass on the target;
* the target's own tests and requirements still pass;
* no tests were deleted or weakened;
* no files the target deleted were brought back;
* no definitions the target has were dropped;
* for reverts, the behaviour is really gone.

It works on PRs made by people, bots or AI agents, and calls no AI.

## Usage

```yaml
# .github/workflows/verify-backports.yml
name: verify backports
on:
  pull_request:
    branches: ["stable/**", "release/**"]

permissions:
  contents: read
  pull-requests: write        # only needed for the PR comment

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0       # the original commits must be reachable
      - uses: actions/setup-python@v5        # your project's own test setup
        with: { python-version: "3.12" }
      - run: pip install -e . pytest
      - uses: eladb3/gitent@v0
        with:
          test-command: "python -m pytest -q"
          test-selector: "python -m pytest -q -k {test}"
```

## How it finds the original change

The action reads the PR's commits (`base..head`) and looks for these, in order:

| marker | written by |
|---|---|
| `Gitent-Derived-From: <sha>` | `gitent cherry-pick` / `rebase` |
| `(cherry picked from commit <sha>)` | `git cherry-pick -x`, patchback and most backport bots |
| `Backport of <sha>` / `Backported from <sha>` | Django-style conventions |
| `This reverts commit <sha>.` | `git revert` (verified with `--kind revert`) |

Or pass `source: <sha>` explicitly. A PR with no marker is reported as
**neutral**: the action passes, with a note. Set `require-source: true` to fail
instead.

## Inputs

| input | default | meaning |
|---|---|---|
| `base` | the PR's base branch | target before the change (`origin/<base_ref>`) |
| `head` | `HEAD` | the result to verify |
| `source` | auto-detect | the original change (sha, ref or `int_…`) |
| `kind` | auto (`apply`, or `revert` for revert commits) | `apply`, `revert` or `merge` |
| `test-command` | from `.gitent/config.toml` | runs the project's tests |
| `test-selector` | from `.gitent/config.toml` | runs one test; `{test}` is replaced by its name |
| `require-source` | `false` | fail if no original change is found |
| `comment` | `true` on `pull_request` events | maintain one sticky PR comment |
| `version` | the action's own tag | Gitent version to install |

## Outputs

| output | meaning |
|---|---|
| `result` | `verified`, `rejected`, `neutral` or `error` |
| `report` | path to the Markdown report |
| `json` | path to the JSON result |

## Exit status

`verified` and `neutral` pass the job; `rejected` and `error` fail it. Use the
`result` output with `continue-on-error: true` to make the check advisory.

## Local equivalent

```bash
gitent verify-pr --base origin/stable/5.2.x --head HEAD --markdown report.md
```

## Limits

* Gitent checks what is recorded, plus the tests. Behaviour that only matters
  on the target branch, and that no test or requirement on that branch covers,
  cannot be caught.
* The original commit must be in the clone (use `fetch-depth: 0`).
