# Gitent for Claude Code

This plugin adds the `gitent` MCP server (`gitent mcp`) and a skill that tells
Claude Code when to use it. With it, Claude records why each change exists and
what must stay true, and verifies backports, cherry-picks, merges and reverts
before it reports "done".

## Install

1. Install the `gitent` binary (Linux and macOS):

   ```bash
   curl -fsSL https://raw.githubusercontent.com/eladb3/gitent/main/install.sh | sh
   ```

   Or `cargo install gitent`. The MCP server starts `gitent` from your `PATH`.

2. Add the plugin in Claude Code:

   ```text
   /plugin marketplace add eladb3/gitent
   /plugin install gitent@gitent
   ```

3. In your repository, run `gitent init` once. It detects your test command.

## What Claude gets

* `gitent_verify`: checks a port (a commit, a branch or uncommitted changes)
  against the original change's requirements and the target's own tests.
* `gitent_commit`: a commit with its intent and requirements recorded and proven.
* `gitent_cherry_pick`, `gitent_merge`, `gitent_rebase`, `gitent_revert`: Git
  first; otherwise Claude edits an isolated worktree and Gitent validates it.
* `gitent_check`, `gitent_log`, `gitent_show`: what the code base has promised, and why.

See the main [README](../../README.md) for what `verify` checks and what it cannot see.
