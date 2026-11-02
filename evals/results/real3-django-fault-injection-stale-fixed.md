### Run 2026-11-02T22-49-45Z — agent: `stale`, gitent LLM: `none`

| case | baseline | gitent |
|---|---|---|
| stable-5.2.x-02a69e3791 | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-5.2.x-6ecd66e09a | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-5.2.x-721685aa77 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-5.2.x-9b62b0af71 | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-5.2.x-b9adb81339 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-5.2.x-c72a5dbb64 | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-5.2.x-d5d60ed032 | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-5.2.x-ff27883ca0 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-6.0.x-0910af6046 | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-6.0.x-3d32ee80ae | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-6.0.x-3d8f121695 | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-6.0.x-4b86ba51e4 | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-6.0.x-972dbdd4f7 | ✅ reported success | ❌ reported failure, agent calls: 2 |
| stable-6.0.x-ad8f9e19e0 | ✅ reported success | ✅ reported success, agent calls: 0 |
| stable-6.0.x-b83ab92bc5 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-6.1.x-0931d3c352 | ❌ reported success ⚠️ FALSE SUCCESS | ✅ reported success, agent calls: 0 |
| stable-6.1.x-3143a7e7a7 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-6.1.x-8485c6ac38 | ✅ reported success | ✅ reported success, agent calls: 0 |
| stable-6.1.x-8590dfdb41 | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-6.1.x-969450ebcc | ✅ reported success | ✅ reported success, agent calls: 1 |
| stable-6.1.x-a92be5c4e2 | ❌ reported success ⚠️ FALSE SUCCESS | ✅ reported success, agent calls: 0 |
| stable-6.1.x-bc529fb8a6 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-6.1.x-cabad83ed1 | ❌ reported success ⚠️ FALSE SUCCESS | ❌ reported failure, agent calls: 2 |
| stable-6.1.x-cce6c57ed3 | ✅ reported success | ✅ reported success, agent calls: 0 |

| arm | hidden pass | false successes |
|---|---|---|
| baseline | 15/24 | 9 |
| gitent | 11/24 | 0 |

| kind | baseline | gitent |
|---|---|---|
| identical | 2/4 | 4/4 |
| clean_differs | 1/1 | 1/1 |
| conflict | 12/19 | 6/19 |
