//! Any coding-agent CLI as an executor (Claude Code, Codex, Kiro, Gemini, aider, scripts…).

use super::{ExecReport, Executor, Task};
use anyhow::Result;
use std::time::Duration;

pub struct CommandExecutor {
    pub command: String,
    pub timeout_secs: u64,
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Executor for CommandExecutor {
    fn name(&self) -> String {
        format!("command:{}", self.command)
    }

    fn execute(&self, task: &Task) -> Result<ExecReport> {
        let brief_path = task.brief_path.to_string_lossy().to_string();
        let cmd = self.command.replace("{brief}", &shell_quote(&brief_path));
        let env = [
            ("GITENT_BRIEF", brief_path.clone()),
            (
                "GITENT_WORKTREE",
                task.worktree.to_string_lossy().to_string(),
            ),
            ("GITENT_OP_ID", task.op_id.clone()),
            ("GITENT_ATTEMPT", task.attempt.to_string()),
            ("GITENT_ROLE", "executor".to_string()),
        ];
        // Agent CLIs need their own API keys, never GitHub tokens (ADR 0016 §1).
        let out = crate::proc::run_shell_with(
            crate::proc::EnvPolicy::LlmCli,
            &cmd,
            &task.worktree,
            Some(&task.brief),
            &env,
            Duration::from_secs(self.timeout_secs),
        )?;
        let summary = if out.timed_out {
            format!("agent timed out after {}s", self.timeout_secs)
        } else if out.success() {
            crate::proc::tail(out.stdout.trim(), 2000)
        } else {
            format!("agent exited with code {:?}", out.code)
        };
        Ok(ExecReport {
            executor: self.name(),
            success: out.success(),
            summary,
            log: out.combined_tail(20_000),
        })
    }
}
