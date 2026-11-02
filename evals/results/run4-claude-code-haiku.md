### Run 2026-11-02T17-54-44Z — agent: `claude-code-haiku`, gitent LLM: `claude -p --model claude-haiku-5-5`

| case | git | baseline | gitent |
|---|---|---|---|
| E0-parity-clean-pick | ✅ reported success | ✅ reported success | ✅ reported success, agent calls: 0 |
| E1-pick-after-rename-move | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |
| E2-pick-after-signature-change | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |
| E3-pick-clean-but-wrong | ❌ reported success ⚠️ FALSE SUCCESS | ✅ reported success | ✅ reported success, agent calls: 1 |
| E4-merge-both-intentions | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |
| E5-revert-after-refactor | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |
| E6-rebase-stack-onto-refactor | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 3 |
| E7-pick-into-target-invariant | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |
| E8-pick-multi-site-intent | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 1 |

| arm | hidden pass | false successes |
|---|---|---|
| git | 1/9 | 1 |
| baseline | 9/9 | 0 |
| gitent | 9/9 | 0 |
