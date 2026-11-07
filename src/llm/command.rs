//! Any CLI as an LLM: prompt on stdin, reply on stdout (e.g. `claude -p`, `codex exec -`).

use super::{flatten, Message, Provider};
use crate::config::LlmConfig;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct CommandProvider {
    command: String,
    cwd: PathBuf,
    timeout: u64,
}

impl CommandProvider {
    pub fn new(cfg: &LlmConfig, cwd: &Path) -> Self {
        CommandProvider {
            command: cfg.command.clone(),
            cwd: cwd.to_path_buf(),
            timeout: cfg.timeout_secs,
        }
    }
}

impl Provider for CommandProvider {
    fn name(&self) -> String {
        format!("command:{}", self.command)
    }

    /// The CLI runs in an EMPTY temporary directory: the repository (possibly a PR
    /// checkout) may carry `.claude/settings.json`, `.mcp.json` or `AGENTS.md` that would
    /// configure an agentic CLI (ADR 0015 §6). Everything it needs is in the prompt.
    fn complete(&self, system: &str, messages: &[Message]) -> Result<String> {
        let _ = &self.cwd;
        let empty = tempfile::tempdir()?;
        self.complete_in(system, messages, empty.path())
    }

    fn complete_in(&self, system: &str, messages: &[Message], dir: &Path) -> Result<String> {
        let prompt = flatten(system, messages);
        // The CLI needs its own API key, never GitHub tokens (ADR 0016 §1).
        let out = crate::proc::run_shell_with(
            crate::proc::EnvPolicy::LlmCli,
            &self.command,
            dir,
            Some(&prompt),
            &[("GITENT_ROLE", "llm".to_string())],
            Duration::from_secs(self.timeout),
        )?;
        if !out.success() {
            bail!(
                "llm command `{}` failed (code {:?}, timed_out={}): {}",
                self.command,
                out.code,
                out.timed_out,
                out.combined_tail(800)
            );
        }
        if out.stdout.trim().is_empty() {
            bail!("llm command `{}` produced no output", self.command);
        }
        Ok(out.stdout)
    }
}
