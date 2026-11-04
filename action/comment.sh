#!/usr/bin/env bash
# Create or update ONE sticky comment (found by the hidden marker) with the report.
set -uo pipefail
marker='<!-- gitent-verify-pr -->'
repo="${GITHUB_REPOSITORY}"
id=$(gh api "repos/$repo/issues/$PR_NUMBER/comments" --paginate \
       --jq ".[] | select(.body | contains(\"$marker\")) | .id" 2>/dev/null | head -1)
body=$(python3 -c 'import json,sys; print(json.dumps({"body": open(sys.argv[1]).read()}))' "$REPORT")
if [ -n "$id" ]; then
  echo "$body" | gh api -X PATCH "repos/$repo/issues/comments/$id" --input - >/dev/null \
    && echo "gitent: updated comment $id" || echo "::warning::gitent: could not update the PR comment (needs pull-requests: write)"
else
  echo "$body" | gh api -X POST "repos/$repo/issues/$PR_NUMBER/comments" --input - >/dev/null \
    && echo "gitent: posted comment" || echo "::warning::gitent: could not post a PR comment (needs pull-requests: write)"
fi
exit 0
