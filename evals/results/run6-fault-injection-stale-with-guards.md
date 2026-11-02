### Run 2026-11-02T18-01-32Z — agent: `stale`, gitent LLM: `none`

| case | baseline | gitent |
|---|---|---|
| E0-parity-clean-pick | ✅ reported success | ✅ reported success, agent calls: 0 |
| E1-pick-after-rename-move | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E2-pick-after-signature-change | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E3-pick-clean-but-wrong | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E4-merge-both-intentions | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E5-revert-after-refactor | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E6-rebase-stack-onto-refactor | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E7-pick-into-target-invariant | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| E8-pick-multi-site-intent | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |

| arm | hidden pass | false successes |
|---|---|---|
| baseline | 1/9 | 8 |
| gitent | 1/9 | 0 |
