//! Executors: things that change code in a worktree given a task brief (ADR 0004).

mod builtin;
mod command;

use crate::config::Config;
use crate::llm::Provider;
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::sync::Arc;

pub struct Task {
    pub op_id: String,
    pub attempt: u32,
    pub worktree: PathBuf,
    pub brief_path: PathBuf,
    pub brief: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ExecReport {
    pub executor: String,
    pub success: bool,
    pub summary: String,
    /// Tail of the agent's transcript/output (kept in the op journal).
    pub log: String,
}

pub trait Executor {
    fn name(&self) -> String;
    fn execute(&self, task: &Task) -> Result<ExecReport>;
}

pub enum ExecutorChoice {
    /// The calling agent does the work (pause → `gitent continue`).
    Host,
    Run(Box<dyn Executor>),
}

impl ExecutorChoice {
    pub fn name(&self) -> String {
        match self {
            ExecutorChoice::Host => "host".into(),
            ExecutorChoice::Run(e) => e.name(),
        }
    }
}

pub fn from_config(cfg: &Config, provider: Option<Arc<dyn Provider>>) -> Result<ExecutorChoice> {
    Ok(match cfg.executor.kind.as_str() {
        "" | "host" => ExecutorChoice::Host,
        "command" => {
            if cfg.executor.command.trim().is_empty() {
                bail!("executor.kind = \"command\" requires executor.command");
            }
            ExecutorChoice::Run(Box::new(command::CommandExecutor {
                command: cfg.executor.command.clone(),
                timeout_secs: cfg.executor.timeout_secs,
            }))
        }
        "builtin" => {
            let Some(p) = provider else {
                bail!("executor.kind = \"builtin\" requires an LLM provider (llm.provider)");
            };
            ExecutorChoice::Run(Box::new(builtin::BuiltinAgent {
                provider: p,
                max_steps: cfg.executor.max_steps,
                command_timeout: cfg.validation.timeout_secs,
            }))
        }
        other => bail!("unknown executor kind '{other}' (expected builtin|command|host)"),
    })
}
