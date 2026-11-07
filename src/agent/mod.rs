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

/// One-line warning for executor commands known to run an agent with every
/// tool auto-approved and no sandbox (the `gemini`, `kiro` and `aider` presets).
pub fn unsandboxed_warning(command: &str) -> Option<String> {
    let c = command.trim_start();
    let (agent, flag) = if c.starts_with("gemini") && c.contains("--yolo") {
        ("gemini", "--yolo")
    } else if c.starts_with("kiro-cli") && c.contains("--trust-all-tools") {
        ("kiro", "--trust-all-tools")
    } else if c.starts_with("aider") && c.contains("--yes-always") {
        ("aider", "--yes-always")
    } else {
        return None;
    };
    Some(format!(
        "the {agent} executor runs with all tools auto-approved ({flag}) and unsandboxed: it can run any command as you; use it only on code you trust"
    ))
}

/// The builtin agent's `run` tool executes shell commands the LLM chooses (ADR 0016 §6).
pub const BUILTIN_WARNING: &str = "the builtin executor's `run` tool executes shell commands chosen by the LLM, unsandboxed (with a scrubbed environment): use it only on code you trust";

/// The warning for an executor configuration, if it runs unsandboxed.
pub fn executor_warning(cfg: &crate::config::ExecutorConfig) -> Option<String> {
    match cfg.kind.as_str() {
        "builtin" => Some(BUILTIN_WARNING.to_string()),
        "command" => unsandboxed_warning(&cfg.command),
        _ => None,
    }
}

fn warn_once(w: &str) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| eprintln!("gitent: warning: {w}"));
}

pub fn from_config(cfg: &Config, provider: Option<Arc<dyn Provider>>) -> Result<ExecutorChoice> {
    Ok(match cfg.executor.kind.as_str() {
        "" | "host" => ExecutorChoice::Host,
        "command" => {
            if cfg.executor.command.trim().is_empty() {
                bail!(
                    "executor.kind = \"command\" requires executor.command (if .gitent/config.toml sets it, review it and run `gitent trust`)"
                );
            }
            if let Some(w) = executor_warning(&cfg.executor) {
                warn_once(&w);
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
            warn_once(BUILTIN_WARNING);
            ExecutorChoice::Run(Box::new(builtin::BuiltinAgent {
                provider: p,
                max_steps: cfg.executor.max_steps,
                command_timeout: cfg.validation.timeout_secs,
            }))
        }
        other => bail!("unknown executor kind '{other}' (expected builtin|command|host)"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsandboxed_presets_warn() {
        for (p, warn) in [
            ("gemini", true),
            ("kiro", true),
            ("aider", true),
            ("builtin", true),
            ("openai", true),
            ("claude-code", false),
            ("codex", false),
            ("host", false),
        ] {
            let c = Config::preset(p).unwrap();
            let w = executor_warning(&c.executor);
            assert_eq!(w.is_some(), warn, "{p}: {w:?}");
            if let Some(w) = w {
                assert!(w.contains("unsandboxed") && !w.contains('\n'), "{w}");
            }
        }
    }
}
