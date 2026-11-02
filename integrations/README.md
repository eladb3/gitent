# Integrations

| Agent | Files | Install |
|---|---|---|
| Claude Code | `claude-code/.mcp.json`, `claude-code/skills/gitent/SKILL.md` | `claude mcp add gitent -- gitent mcp`; copy the skill to `.claude/skills/gitent/` |
| Codex | `codex/config.toml`, `codex/AGENTS.md` | append the TOML to `~/.codex/config.toml`; append `AGENTS.md` to your repo's |
| Kiro | `kiro/mcp.json`, `kiro/steering/gitent.md` | copy to `.kiro/settings/mcp.json` and `.kiro/steering/` |
| Anything else | `AGENT_GUIDE.md` | point the agent at `gitent mcp`, `gitent serve`, or `gitent --json` |

These are configuration files. They have not yet been exercised against each CLI in CI.
