# Examples

| File | What it is |
|---|---|
| [`prove-tests.yml`](prove-tests.yml) | GitHub workflow for **every** PR: each new/changed test must fail without the PR's code and pass with it. |
| [`verify-backports.yml`](verify-backports.yml) | GitHub workflow: verify every backport PR into `stable/**` / `release/**` (Python). |
| [`verify-backports-node.yml`](verify-backports-node.yml) | The same for a Node / Jest project. |
| [`require-source.yml`](require-source.yml) | Strict release gate: PRs must name the change they port. Also shows the action's `result` output. |
| [`agent-port.sh`](agent-port.sh) | Local loop: an agent ports a fix, `gitent verify` decides whether to commit it. |

## Everyday commands

```bash
gitent init                                  # once per repo; detects the test command
gitent commit -m "fix: cache user lookups"   # records intent + proven requirements
gitent verify <fix>                          # is HEAD a correct port of <fix>?
gitent verify <fix> --working-tree           # ...before committing an agent's edits
gitent verify <fix> --onto main              # a whole branch against its target
gitent verify <fix> --kind revert            # is the behaviour really gone?
gitent prove-pr main                         # do this PR's tests test its change?
gitent prove-pr main --working-tree          # ...before committing (agents: do this before "done")
gitent verify-pr --base main                 # what the backport Action runs
gitent check                                 # run every live requirement
gitent log                                   # history as intentions + provenance
```

Add `--json` to any command for machine-readable output.

## See it run

[`docs/demo/demo.sh`](../docs/demo/demo.sh) builds a throwaway repo and plays the whole
story (conflicting cherry-pick → stale agent port rejected → proper port verified).
Run it with `DEMO_SPEED=0 docs/demo/demo.sh` for no typing delay.
