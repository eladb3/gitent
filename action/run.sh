#!/usr/bin/env bash
# Run `gitent verify-pr` and expose the result. Never fails by itself; the "Result" step decides.
set -uo pipefail
cd "${INPUT_WORKING_DIRECTORY:-.}" || { echo "::error::gitent: working-directory not found"; echo "result=error" >> "$GITHUB_OUTPUT"; exit 0; }
out="${RUNNER_TEMP:-/tmp}"
report="$out/gitent-report.md"
json="$out/gitent-result.json"
rm -f "$report" "$json"

base="${INPUT_BASE:-}"
if [ -z "$base" ]; then
  if [ -z "${GITENT_BASE_REF:-}" ]; then
    echo "::error::gitent: no 'base' input and not a pull_request event"
    echo "result=error" >> "$GITHUB_OUTPUT"; exit 0
  fi
  base="origin/${GITENT_BASE_REF}"
  git rev-parse --verify -q "$base" >/dev/null || git fetch --no-tags -q origin "${GITENT_BASE_REF}:refs/remotes/origin/${GITENT_BASE_REF}" || true
fi
# Shared semantic records, if the remote has them (harmless otherwise).
git fetch --no-tags -q origin 'refs/notes/gitent:refs/notes/gitent' 2>/dev/null || true
git config user.email >/dev/null || git config user.email "gitent@users.noreply.github.com"
git config user.name  >/dev/null || git config user.name  "gitent"

args=(--json)
[ -n "${INPUT_TEST_COMMAND:-}" ]  && args+=(--test-command "$INPUT_TEST_COMMAND")
[ -n "${INPUT_TEST_SELECTOR:-}" ] && args+=(--test-selector "$INPUT_TEST_SELECTOR")
args+=(verify-pr --base "$base" --head "${INPUT_HEAD:-HEAD}" --markdown "$report" --no-llm)
[ -n "${INPUT_SOURCE:-}" ] && args+=(--source "$INPUT_SOURCE")
[ -n "${INPUT_KIND:-}" ]   && args+=(--kind "$INPUT_KIND")
[ "${INPUT_REQUIRE_SOURCE:-false}" = "true" ] && args+=(--require-source)

echo "gitent ${args[*]}"
gitent "${args[@]}" > "$json" 2> "$out/gitent-stderr.txt"
code=$?
result=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("result","error"))' "$json" 2>/dev/null || echo error)
[ "$code" = "1" ] && result="error"

if [ ! -s "$report" ]; then
  err=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error",""))' "$json" 2>/dev/null || true)
  [ -z "$err" ] && err=$(cat "$json" "$out/gitent-stderr.txt" 2>/dev/null | tail -40)
  printf '<!-- gitent-verify-pr -->\n### ⚠️ Gitent: could not verify\n\n```\n%s\n```\n' "$err" > "$report"
fi
cat "$report" >> "$GITHUB_STEP_SUMMARY"
[ "$result" = "rejected" ] && echo "::error::gitent: backport rejected (see the job summary)"
[ "$result" = "error" ] && echo "::error::gitent: verification error (see the job summary)"
{
  echo "result=$result"
  echo "report=$report"
  echo "json=$json"
} >> "$GITHUB_OUTPUT"
exit 0
