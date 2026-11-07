#!/usr/bin/env bash
# Run `gitent verify-pr` and expose the result. Never fails by itself; the "Result" step decides.
#
# Exit-code contract of `gitent verify-pr` (mapped explicitly, ADR 0014 section 9):
#   0 = verified or neutral, 2 = rejected, 3 = unverified (nothing proved the
#   fix; needs human review), anything else = error.
set -uo pipefail
# This step runs the PR's code (its tests). GitHub tokens belong to the comment
# step only (ADR 0016 section 1); gitent also scrubs its children's environment.
unset GH_TOKEN GITHUB_TOKEN GITENT_TOKEN
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${RUNNER_TEMP:-/tmp}"
report="$out/gitent-report.md"
json="$out/gitent-result.json"
errlog="$out/gitent-stderr.txt"
rm -f "$report" "$json" "$errlog"

emit() {
  {
    echo "result=$1"
    echo "report=$report"
    echo "json=$json"
  } >> "$GITHUB_OUTPUT"
}

# Report an error the action itself detected (before or instead of gitent's report).
fail_early() {
  echo "::error::gitent: $1"
  printf '%s\n' "$1" > "$errlog"
  { printf '<!-- gitent-verify-pr -->\n### ⚠️ Gitent: could not verify\n\n'; python3 -I "$here/md.py" fence "$errlog"; } > "$report"
  cat "$report" >> "$GITHUB_STEP_SUMMARY"
  emit error
  exit 0
}

cd "${INPUT_WORKING_DIRECTORY:-.}" || fail_early "working-directory not found"

# actions/checkout persists the job token in .git/config unless told not to;
# the PR's tests can read it from there.
if git config --get-regexp '^http\..*\.extraheader$' >/dev/null 2>&1; then
  echo "::warning::gitent: the checkout persisted credentials in .git/config, where the PR's tests can read them; use actions/checkout with 'persist-credentials: false'"
fi

base="${INPUT_BASE:-}"
if [ -z "$base" ]; then
  [ -z "${GITENT_BASE_REF:-}" ] && fail_early "no 'base' input and not a pull_request event"
  base="origin/${GITENT_BASE_REF}"
  git rev-parse --verify -q "$base" >/dev/null || git fetch --no-tags -q origin "${GITENT_BASE_REF}:refs/remotes/origin/${GITENT_BASE_REF}" || true
fi

# Shared semantic records. A remote without the notes ref is normal and silent;
# a failure to query or fetch it is reported, since results may then differ.
if git remote get-url origin >/dev/null 2>&1; then
  git ls-remote --exit-code origin refs/notes/gitent >/dev/null 2>"$out/gitent-notes.txt"
  lr=$?
else
  lr=2 # no remote at all (local checkouts, the self-test fixtures): nothing to fetch
fi
case "$lr" in
  0)
    if ! git fetch --no-tags -q origin 'refs/notes/gitent:refs/notes/gitent' 2>>"$out/gitent-notes.txt"; then
      echo "::warning::gitent: could not fetch refs/notes/gitent from origin; verifying without the shared semantic records: $(tail -3 "$out/gitent-notes.txt" | tr '\n' ' ')"
    fi
    ;;
  2) ;; # the remote has no gitent notes
  *) echo "::warning::gitent: could not query origin for refs/notes/gitent (exit $lr); verifying without the shared semantic records: $(tail -3 "$out/gitent-notes.txt" | tr '\n' ' ')" ;;
esac
git config user.email >/dev/null || git config user.email "gitent@users.noreply.github.com"
git config user.name  >/dev/null || git config user.name  "gitent"

args=(--json)
[ -n "${INPUT_TEST_COMMAND:-}" ]  && args+=(--test-command "$INPUT_TEST_COMMAND")
[ -n "${INPUT_TEST_SELECTOR:-}" ] && args+=(--test-selector "$INPUT_TEST_SELECTOR")

# Optional LLM verifier (off by default). Keys come from the step's env.
provider="${INPUT_LLM_PROVIDER:-}"
case "$provider" in
  "") ;;
  anthropic)
    [ -n "${ANTHROPIC_API_KEY:-}" ] || fail_early "llm-provider: anthropic needs ANTHROPIC_API_KEY in the step's env (env: ANTHROPIC_API_KEY: \${{ secrets.ANTHROPIC_API_KEY }})" ;;
  openai)
    [ -n "${OPENAI_API_KEY:-}" ] || fail_early "llm-provider: openai needs OPENAI_API_KEY in the step's env (env: OPENAI_API_KEY: \${{ secrets.OPENAI_API_KEY }})" ;;
  *) fail_early "unsupported llm-provider '$provider' (expected anthropic or openai)" ;;
esac
if [ -n "$provider" ]; then
  args+=(--provider "$provider")
  [ -n "${INPUT_LLM_MODEL:-}" ] && export GITENT_MODEL="$INPUT_LLM_MODEL"
fi

args+=(verify-pr --base "$base" --head "${INPUT_HEAD:-HEAD}" --markdown "$report")
[ -z "$provider" ] && args+=(--no-llm)
[ -n "${INPUT_SOURCE:-}" ] && args+=(--source "$INPUT_SOURCE")
[ -n "${INPUT_KIND:-}" ]   && args+=(--kind "$INPUT_KIND")
[ "${INPUT_REQUIRE_SOURCE:-false}" = "true" ] && args+=(--require-source)

echo "gitent ${args[*]}"
gitent "${args[@]}" > "$json" 2> "$errlog"
code=$?
reported=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("result",""))' "$json" 2>/dev/null || true)

case "$code" in
  0)
    case "$reported" in
      verified|neutral) result="$reported" ;;
      *) result="error"; echo "::error::gitent: exit code 0 but result '${reported}'" ;;
    esac ;;
  2) result="rejected" ;;
  3) result="unverified" ;;
  *) result="error" ;;
esac

if [ ! -s "$report" ]; then
  err=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1])).get("error",""))' "$json" 2>/dev/null || true)
  if [ -n "$err" ]; then printf '%s\n' "$err" > "$out/gitent-error.txt"
  else cat "$json" "$errlog" 2>/dev/null | tail -40 > "$out/gitent-error.txt"; fi
  { printf '<!-- gitent-verify-pr -->\n### ⚠️ Gitent: could not verify (exit code %s)\n\n' "$code"; python3 -I "$here/md.py" fence "$out/gitent-error.txt"; } > "$report"
fi
cat "$report" >> "$GITHUB_STEP_SUMMARY"
case "$result" in
  rejected)   echo "::error::gitent: backport rejected (see the job summary)" ;;
  unverified) echo "::warning::gitent: backport unverified: nothing proved the change; needs human review (see the job summary)" ;;
  error)      echo "::error::gitent: verification error, exit code $code (see the job summary)" ;;
esac
emit "$result"
exit 0
