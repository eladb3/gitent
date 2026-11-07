# GitHub Action: verify backport pull requests

The action checks that a backport, cherry-pick or revert PR correctly carries
the original change onto its target branch:

* the change's own tests and requirements pass on the target;
* the target's own tests and requirements still pass;
* no tests were deleted or weakened;
* no files the target deleted were brought back;
* no definitions the target has were dropped;
* for reverts, the behaviour is really gone.

It works on PRs made by people, bots or AI agents, and calls no AI unless
you opt in to the LLM verifier (`llm-provider`).

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
| `allow-unverified` | `false` | `unverified` result: `false` fails the job, `true` passes it with a warning |
| `llm-provider` | empty (off: `--no-llm`) | opt-in LLM verifier: `anthropic` or `openai` (key from the step's `env`, see below) |
| `llm-model` | provider default | model for the LLM verifier |
| `comment` | `true` on `pull_request` events | maintain one sticky PR comment |
| `version` | the action's own tag | Gitent version to install |

## Outputs

| output | meaning |
|---|---|
| `result` | `verified`, `rejected`, `unverified`, `neutral` or `error` |
| `report` | path to the Markdown report |
| `json` | path to the JSON result |

## Exit status

`gitent verify-pr` exits `0` (verified or neutral), `2` (rejected), `3`
(unverified: nothing proved the change, it needs human review) or anything
else (error); the action maps these codes explicitly.

`verified` and `neutral` pass the job; `rejected` and `error` fail it;
`unverified` fails it unless `allow-unverified: "true"`, which passes with a
warning. Use the `result` output with `continue-on-error: true` to make the
check advisory.

## Opt-in LLM verifier

```yaml
      - uses: eladb3/gitent@v0
        env:
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}   # or OPENAI_API_KEY for openai
        with:
          llm-provider: anthropic
          llm-model: ""            # optional
          test-command: "python -m pytest -q"
```

Without `llm-provider` the action passes `--no-llm` and needs no secrets. With
it, `verify-pr` runs with `--provider <name>`; a missing key is an `error`.
Secrets are not available to `pull_request` runs from forks; do not switch to
`pull_request_target` to get them, because the job runs the PR's code (its
tests) and would then run it with your secrets. Base URLs come from the
environment (`GITENT_LLM_BASE_URL`, `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`)
before config, and keys are never sent over plain `http://` except to
loopback.

## Security notes (ADR 0014)

* **Sticky comment.** The comment is found by its hidden marker *and* its
  author: the token's user (`gh api user`), or `github-actions[bot]` for the
  default `GITHUB_TOKEN`. A comment someone else wrote with the marker is
  never edited. (With a GitHub App installation token `gh api user` is not
  available and the author is assumed to be `github-actions[bot]`, so a new
  comment is posted each run.)
* **Escaping.** The PR title is escaped before it goes into the comment (no
  formatting, links, HTML or @-mentions), and tool output in error reports is
  put in a code fence longer than any backtick run inside it.
* **Release binaries** are checked against the release's published
  `gitent-<target>.tar.gz.sha256`. A mismatch fails the job; a release without
  a checksum asset is not trusted and the action builds from source instead.
* **Semantic records** (`refs/notes/gitent`) are fetched from `origin` when it
  has them. A remote without the ref is normal and silent; a failure to query
  or fetch it is reported with `::warning::`.
* The job runs the PR's own tests, so it runs the PR's code: give it a
  read-only token (`contents: read`; `pull-requests: write` only for the
  comment) and no other secrets.

## Local equivalent

```bash
gitent verify-pr --base origin/stable/5.2.x --head HEAD --markdown report.md
```

## Limits

* Gitent checks what is recorded, plus the tests. Behaviour that only matters
  on the target branch, and that no test or requirement on that branch covers,
  cannot be caught.
* The original commit must be in the clone (use `fetch-depth: 0`).
