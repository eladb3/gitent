//! Gitent's own bounded coding agent: a JSON-action tool loop over any Provider (ADR 0005).

use super::{ExecReport, Executor, Task};
use crate::llm::{extract_json, Message, Provider};
use crate::tools::{Workspace, TOOL_DOCS};
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;

pub struct BuiltinAgent {
    pub provider: Arc<dyn Provider>,
    pub max_steps: u32,
    pub command_timeout: u64,
}

fn system_prompt() -> String {
    format!(
        "You are Gitent's coding agent. You realise a semantic INTENTION in a code base whose structure may \
differ from the code where the intention was first implemented. Investigate the current code first, \
then make minimal, focused edits. Do not delete or weaken tests. Do not commit.\n\n{TOOL_DOCS}\n\n\
Reply with exactly one JSON object per turn and nothing else."
    )
}

impl Executor for BuiltinAgent {
    fn name(&self) -> String {
        format!("builtin:{}", self.provider.name())
    }

    fn execute(&self, task: &Task) -> Result<ExecReport> {
        let ws = Workspace {
            root: task.worktree.clone(),
            command_timeout: Duration::from_secs(self.command_timeout),
        };
        let system = system_prompt();
        let mut msgs = vec![Message::user(task.brief.clone())];
        let mut log = String::new();
        for step in 0..self.max_steps {
            let reply = self.provider.complete(&system, &msgs)?;
            log.push_str(&format!(
                "\n--- step {step} assistant ---\n{}\n",
                crate::proc::tail(&reply, 2000)
            ));
            msgs.push(Message::assistant(reply.clone()));
            let action = match extract_json(&reply) {
                Ok(v) => v,
                Err(_) => {
                    msgs.push(Message::user(
                        "ERROR: reply with exactly one JSON object {\"tool\": ..., \"args\": {...}}".to_string(),
                    ));
                    continue;
                }
            };
            let tool = action["tool"].as_str().unwrap_or("").to_string();
            let args = action.get("args").cloned().unwrap_or(serde_json::json!({}));
            if tool == "finish" {
                let summary = args["summary"].as_str().unwrap_or("").to_string();
                return Ok(ExecReport {
                    executor: self.name(),
                    success: true,
                    summary,
                    log: crate::proc::tail(&log, 20_000),
                });
            }
            let result = match ws.call(&tool, &args) {
                Ok(r) => r,
                Err(e) => format!("ERROR: {e:#}"),
            };
            log.push_str(&format!(
                "--- step {step} {tool} ---\n{}\n",
                crate::proc::tail(&result, 1500)
            ));
            msgs.push(Message::user(format!("TOOL_RESULT {tool}:\n{result}")));
        }
        Ok(ExecReport {
            executor: self.name(),
            success: false,
            summary: format!("step limit ({}) reached without finish", self.max_steps),
            log: crate::proc::tail(&log, 20_000),
        })
    }
}
