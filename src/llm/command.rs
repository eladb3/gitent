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

    fn complete(&self, system: &str, messages: &[Message]) -> Result<String> {
        self.complete_in(system, messages, &self.cwd.clone())
    }

    fn complete_in(&self, system: &str, messages: &[Message], dir: &Path) -> Result<String> {
        let prompt = flatten(system, messages);
        let out = crate::proc::run_shell(
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
