# Integrations

| Agent | Files | Install |
|---|---|---|
| Claude Code | plugin: `claude-code/` (`.mcp.json`, `skills/gitent/SKILL.md`) | `/plugin marketplace add eladb3/gitent`, then `/plugin install gitent@gitent`; or `claude mcp add gitent -- gitent mcp` and copy the skill to `.claude/skills/gitent/` |
| Codex | `codex/config.toml`, `codex/AGENTS.md` | append the TOML to `~/.codex/config.toml`; append `AGENTS.md` to your repo's |
| Kiro | `kiro/mcp.json`, `kiro/steering/gitent.md` | copy to `.kiro/settings/mcp.json` and `.kiro/steering/` |
| Anything else | `AGENT_GUIDE.md` | point the agent at `gitent mcp`, `gitent serve` (send `Authorization: Bearer $GITENT_TOKEN`; see `docs/INTEGRATIONS.md`), or `gitent --json` |

These are configuration files. They have not yet been exercised against each CLI in CI.
