# 0006 — Validation policy

**Status:** accepted

Hard (deterministic) gates: non-empty change, test command passes, tests
introduced by the source change are present, pre-existing tests preserved.

The LLM verifier is *advisory* by default: a negative verdict with confidence ≥
`verifier_threshold` (default 0.7) fails the attempt and its concerns become
feedback; lower-confidence concerns are recorded as warnings.
`validation.require_verifier = true` makes an unavailable verifier a failure.

If no test command is configured, results are committed with a
`no_test_command` warning and `confidence ≤ 0.5`; `--strict` refuses to
finalize with any warning.
