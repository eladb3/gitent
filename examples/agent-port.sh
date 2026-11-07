#!/usr/bin/env bash
# Port a fix with any coding agent, then let gitent decide whether it is real.
# Usage: examples/agent-port.sh <fix-commit> [target-branch]
set -euo pipefail
fix=${1:?fix commit}; target=${2:-main}

git switch -c "port-$(git rev-parse --short "$fix")" "$target"

# 1. Let the agent do the port (Claude Code shown; any agent works).
claude -p "Port commit $fix onto this branch. The code has moved on; adapt the fix and its tests." \
  --permission-mode acceptEdits

# 2. Verify before trusting the agent's "done".
if gitent verify "$fix" --working-tree; then
  git add -A && git commit -qm "port of $fix" -m "(cherry picked from commit $(git rev-parse "$fix"))"
  gitent verify "$fix" --record        # attach provenance to the new commit
else
  echo "gitent rejected the port; see the failed checks above" >&2
  exit 2
fi
