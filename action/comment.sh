#!/usr/bin/env bash
# Create or update ONE sticky comment with the report. The comment is found by
# its hidden marker AND its author (the token's user), so a comment anyone else
# wrote with the same marker is never edited or mistaken for ours (ADR 0014 section 9).
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
marker='<!-- gitent-verify-pr -->'
repo="${GITHUB_REPOSITORY}"

# Who are we? A PAT / app user token answers `gh api user`; the default
# GITHUB_TOKEN cannot, and its comments are authored by github-actions[bot].
me=$(gh api user --jq .login 2>/dev/null || true)
if ! printf '%s' "$me" | grep -Eq '^[A-Za-z0-9][A-Za-z0-9-]*(\[bot\])?$'; then
  me="github-actions[bot]"
fi
echo "gitent: sticky comment author: $me"

id=$(gh api "repos/$repo/issues/$PR_NUMBER/comments" --paginate \
       --jq ".[] | select(.user.login == \"$me\" and (.body | contains(\"$marker\"))) | .id" 2>/dev/null | head -1)

# The report is gitent's own Markdown; the PR title is attacker-controlled text
# and is escaped (no formatting, links, HTML or mentions).
title=""
[ -n "${PR_TITLE:-}" ] && title=$(python3 "$here/md.py" inline "$PR_TITLE" 200)
body=$(python3 -c '
import json, sys
text = open(sys.argv[1], encoding="utf-8", errors="replace").read()
if sys.argv[2]:
    text = text.rstrip("\n") + "\n\n<sub>Pull request: " + sys.argv[2] + "</sub>\n"
print(json.dumps({"body": text}))' "$REPORT" "$title")
if [ -n "$id" ]; then
  echo "$body" | gh api -X PATCH "repos/$repo/issues/comments/$id" --input - >/dev/null \
    && echo "gitent: updated comment $id" || echo "::warning::gitent: could not update the PR comment (needs pull-requests: write)"
else
  echo "$body" | gh api -X POST "repos/$repo/issues/$PR_NUMBER/comments" --input - >/dev/null \
    && echo "gitent: posted comment" || echo "::warning::gitent: could not post a PR comment (needs pull-requests: write)"
fi
exit 0
