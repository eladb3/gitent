### Run 2026-11-02T17-35-42Z — agent: `claude-code`, gitent LLM: `claude -p`

| case | git | baseline | gitent |
|---|---|---|---|
| E0-parity-clean-pick | ✅ reported success | ✅ reported success | ✅ reported success, agent calls: 0 |
| E1-pick-after-rename-move | ❌ reported failure | ✅ reported success | ❌ reported failure, agent calls: 2 |
| E2-pick-after-signature-change | ❌ reported failure | ✅ reported success | ✅ reported success, agent calls: 2 |
| E3-pick-clean-but-wrong | ❌ reported success ⚠️ FALSE SUCCESS | ✅ reported success | ✅ reported success, agent calls: 1 |
| E4-merge-both-intentions | ❌ reported failure | ✅ reported success | ❌ reported failure, agent calls: 2 |
| E5-revert-after-refactor | ❌ reported failure | ✅ reported success | ❌ reported failure, agent calls: 2 |

| arm | hidden pass | false successes |
|---|---|---|
| git | 1/6 | 1 |
| baseline | 6/6 | 0 |
| gitent | 3/6 | 0 |
