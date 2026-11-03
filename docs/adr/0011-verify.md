# 0011: `gitent verify`, validation without driving an agent

**Status:** accepted. **Builds on:** 0003, 0006, 0008, 0010.

## Context

The evals (`evals/RESULTS.md`) separate what Gitent's value is from what it isn't.

* **Not capability.** On 24 real Django backports (19 git conflicts), Claude
  Code alone was correct 24/24, and so was Gitent driving it.
* **Safety.** With a faulty agent, the agent's own report produced 9 false
  successes on real code and 8 on synthetic scenarios. Gitent produced 0.
  Every one of Gitent's extra rejections was a genuinely broken result.
* **Cost.** Driving the agent through Gitent took about 1.9× the wall time.

The safety comes entirely from validation: requirements, test guards,
`target_preserved` and `revert_effective`. It does not depend on Gitent
*running* the agent. Today that validation is only reachable from inside a
Gitent operation.

## Decision

Add `gitent verify`. It checks whether a result that already exists, produced
by any agent, bot or human, correctly moves a change.

```
gitent verify <source> [<result>] [--onto <base>] [--working-tree] [--kind apply|revert|merge] [--record] [--no-llm]
```

| argument | meaning | default |
|---|---|---|
| `<source>` | the change that was moved: a commit or `int_…`. For `merge`, the merged branch. | required |
| `<result>` | the commit to verify | `HEAD` |
| `--onto <base>` | the target as it was before the change. A multi-commit result (e.g. a backport PR) is verified as a whole against it. | first parent of `<result>`; for `merge`, `<result>^1` |
| `--working-tree` | verify uncommitted changes against `HEAD`, the typical case after an agent edited files. The user's index and working tree are not modified. | off |
| `--kind` | `apply` (cherry-pick / backport / rebase step), `revert` or `merge` | `apply` |
| `--record` | if verified, attach a semantic record to `<result>` (`provenance.operation = "verify"`, `derived_from = <source>`, validation evidence) | off |
| `--no-llm` | skip the LLM verifier even if one is configured | off |

**Checks.** The same validation as an operation step, with the same code:

* `non_empty`
* `requirements`: the source's requirements, re-bound by test name, plus `ledger(base)`
* `source_tests_present`, `existing_tests_preserved`
* `tests`: `validation.test_command`
* `target_preserved` (ADR 0010)
* `revert_effective`, for `--kind revert`
* `verifier`, which is optional

Checks run in a temporary worktree of the result, never in the user's tree.

**Output and exit codes.**
* 0 means verified. 2 means rejected; the response names each failed check and
  the requirements it involves. 1 means an error.
* The JSON shape is `{"verified": bool, "state": "succeeded"|"failed", "kind", "source", "base", "result", "validation": ValidationReport, "warnings"}`.

**Surfaces.** CLI (`--json`), MCP tool `gitent_verify`, and HTTP `POST /v1/tools/gitent_verify`.

## Consequences

* Gitent can sit **behind** any agent or backport bot, as a CI step or a
  pre-merge check, without changing how the work is produced. This is the
  cheapest way to get the measured safety benefit. There is no agent call, and
  the LLM verifier is optional.
* The step validation in `ops::engine` is refactored into a reusable
  `validate_candidate`, so operations and `verify` cannot drift apart.
* `--record` turns externally made backports into Gitent realisations
  (provenance plus requirements), so the ledger stays complete even when Gitent
  did not perform the operation.
