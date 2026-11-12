//! Operations and the shared step engine (ADR 0003).

pub mod brief;
pub mod commit;
pub mod containment;
pub mod engine;
pub mod plan;
pub mod prove_pr;
pub mod verify;
pub mod verify_pr;

use crate::config::Config;
use crate::llm::Provider;
use crate::model::ValidationReport;
use crate::repo::Repo;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An opened repository with its configuration and LLM provider.
pub struct Session {
    pub repo: Repo,
    pub cfg: Config,
    pub provider: Option<Arc<dyn Provider>>,
}

#[derive(Default, Clone, Debug)]
pub struct Overrides {
    pub executor: Option<String>,
    pub executor_command: Option<String>,
    pub provider: Option<String>,
    pub test_command: Option<String>,
    pub test_selector: Option<String>,
    pub max_attempts: Option<u32>,
    /// `--trust-agent-commands` (CLI only, ADR 0016 §3).
    pub trust_agent_commands: bool,
}

impl Session {
    pub fn open(path: &Path, ov: &Overrides) -> Result<Session> {
        Session::open_with_config_from(path, ov, None)
    }

    /// Like `open`, but the shared config is read from `rev`'s tree when given
    /// (`verify-pr` uses the PR base, so a PR cannot choose the commands it is checked with).
    pub fn open_with_config_from(
        path: &Path,
        ov: &Overrides,
        rev: Option<&str>,
    ) -> Result<Session> {
        let repo = Repo::discover(path)?;
        let cfg = match rev {
            None => Config::load(&repo.workdir, &repo.git_dir)?,
            Some(r) => {
                let tree = repo.git.find_commit(repo.resolve(r)?)?.tree()?;
                let text = crate::preserve::file_text(&repo, &tree, ".gitent/config.toml");
                Config::load_with_shared(text.as_deref(), &repo.git_dir)?
            }
        };
        Session::with_config(repo, cfg, ov)
    }

    /// For MCP/HTTP callers (ADR 0016 §3): an agent that edits the working tree
    /// must not choose commands by editing `.gitent/config.toml`, so the command
    /// keys (`config::COMMAND_KEYS`) come from the committed HEAD's copy (plus
    /// local config and env); other keys come from the working tree.
    pub fn open_for_agent(path: &Path, ov: &Overrides) -> Result<Session> {
        let repo = Repo::discover(path)?;
        let shared = repo.workdir.join(".gitent/config.toml");
        let wt = if shared.exists() {
            Some(std::fs::read_to_string(&shared)?)
        } else {
            None
        };
        let head = match repo.head_commit()? {
            Some(c) => crate::preserve::file_text(&repo, &c.tree()?, ".gitent/config.toml"),
            None => None,
        };
        let cfg = Config::load_layered(wt.as_deref(), head.as_deref(), &repo.git_dir)?;
        Session::with_config(repo, cfg, ov)
    }

    fn with_config(repo: Repo, mut cfg: Config, ov: &Overrides) -> Result<Session> {
        if let Some(e) = &ov.executor {
            cfg.executor.kind = e.clone();
        }
        if let Some(c) = &ov.executor_command {
            cfg.executor.command = c.clone();
        }
        if let Some(p) = &ov.provider {
            cfg.llm.provider = p.clone();
        }
        if let Some(t) = &ov.test_command {
            cfg.validation.test_command = t.clone();
        }
        if let Some(t) = &ov.test_selector {
            cfg.validation.test_selector = t.clone();
        }
        if let Some(m) = ov.max_attempts {
            cfg.validation.max_attempts = m.max(1);
        }
        if ov.trust_agent_commands {
            cfg.requirements.trust_agent_commands = true;
        }
        let provider = crate::llm::from_config(&cfg.llm, &repo.workdir)?;
        Ok(Session {
            repo,
            cfg,
            provider,
        })
    }

    pub fn provider(&self) -> Option<&dyn Provider> {
        self.provider.as_deref()
    }

    pub fn ops_dir(&self) -> PathBuf {
        self.repo.gitent_dir().join("ops")
    }

    pub fn op_path(&self, id: &str) -> PathBuf {
        self.ops_dir().join(format!("{id}.json"))
    }

    pub fn save_op(&self, op: &Operation) -> Result<()> {
        validate_op_id(&op.id)?;
        std::fs::create_dir_all(self.ops_dir())?;
        let tmp = self.ops_dir().join(format!("{}.json.tmp", op.id));
        std::fs::write(&tmp, serde_json::to_string_pretty(op)?)?;
        std::fs::rename(tmp, self.op_path(&op.id))?;
        Ok(())
    }

    pub fn load_op(&self, id: &str) -> Result<Operation> {
        validate_op_id(id)?;
        let p = self.op_path(id);
        let text =
            std::fs::read_to_string(&p).with_context(|| format!("no such operation {id}"))?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn list_ops(&self) -> Result<Vec<Operation>> {
        let mut out = vec![];
        if let Ok(rd) = std::fs::read_dir(self.ops_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "json").unwrap_or(false) {
                    if let Ok(op) = serde_json::from_str::<Operation>(&std::fs::read_to_string(&p)?)
                    {
                        if validate_op_id(&op.id).is_ok() {
                            out.push(op);
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(out)
    }

    /// The operation to continue/abort: explicit id, or the most recent open one.
    pub fn pick_open_op(&self, id: Option<&str>) -> Result<Operation> {
        if let Some(id) = id {
            return self.load_op(id);
        }
        let open: Vec<Operation> = self
            .list_ops()?
            .into_iter()
            .filter(|o| o.state.is_open())
            .collect();
        match open.last() {
            Some(o) => Ok(o.clone()),
            None => bail!("no operation in progress"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpState {
    Running,
    AwaitingHost,
    Succeeded,
    Failed,
    Aborted,
}

impl OpState {
    pub fn is_open(self) -> bool {
        matches!(self, OpState::Running | OpState::AwaitingHost)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepKind {
    Apply {
        source: String,
    },
    Revert {
        source: String,
    },
    Merge {
        theirs: String,
        theirs_name: String,
        base: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Attempt {
    pub n: u32,
    /// "git" for the deterministic attempt, otherwise the executor name.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<crate::agent::ExecReport>,
    pub validation: ValidationReport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    #[default]
    Pending,
    AwaitingHost,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub kind: StepKind,
    #[serde(default)]
    pub status: StepStatus,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_commit: Option<String>,
    /// Prepared context (persisted so host-mode `continue` can validate).
    #[serde(default)]
    pub tests_before: Vec<String>,
    #[serde(default)]
    pub required_tests: Vec<String>,
    #[serde(default)]
    pub removable_tests: Vec<String>,
    #[serde(default)]
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<serde_json::Value>,
    /// Requirements the result must satisfy (source + ledger), ADR 0008.
    #[serde(default)]
    pub requirements: Vec<crate::requirements::LiveReq>,
    /// Requirements being removed (revert).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retiring: Vec<crate::requirements::LiveReq>,
}

impl Step {
    pub fn new(kind: StepKind) -> Step {
        Step {
            kind,
            status: StepStatus::Pending,
            attempts: vec![],
            result_commit: None,
            tests_before: vec![],
            required_tests: vec![],
            removable_tests: vec![],
            goal: String::new(),
            brief_path: None,
            compatibility: None,
            requirements: vec![],
            retiring: vec![],
        }
    }

    pub fn agent_attempts(&self) -> u32 {
        self.attempts.iter().filter(|a| a.by != "git").count() as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    /// cherry-pick | revert | merge | rebase
    pub kind: String,
    pub state: OpState,
    pub branch_ref: String,
    pub start_head: Option<String>,
    /// Current tip of the worktree (advances as steps commit).
    pub tip: String,
    pub worktree: String,
    pub executor: String,
    pub steps: Vec<Step>,
    pub current: usize,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub use_git: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_commit: Option<String>,
    pub created_at: String,
}

impl Operation {
    /// JSON for API consumers: everything except bulky agent logs.
    pub fn summary(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(steps) = v.get_mut("steps").and_then(|s| s.as_array_mut()) {
            for s in steps {
                if let Some(atts) = s.get_mut("attempts").and_then(|a| a.as_array_mut()) {
                    for a in atts {
                        if let Some(e) = a.get_mut("exec").and_then(|e| e.as_object_mut()) {
                            e.remove("log");
                        }
                    }
                }
            }
        }
        if self.state == OpState::AwaitingHost {
            if let Some(step) = self.steps.get(self.current) {
                v["brief"] = serde_json::json!(step.brief_path);
                v["next"] = serde_json::json!(format!(
                    "Edit files in {} to satisfy the brief, then run `gitent continue {}` (or `gitent abort {}`).",
                    self.worktree, self.id, self.id
                ));
            }
        }
        v
    }
}

/// Operation ids come from callers (CLI, MCP, HTTP) and are joined into paths:
/// they must match `^op_[A-Za-z0-9_-]{1,64}$` (ADR 0014 §3).
pub fn validate_op_id(id: &str) -> Result<()> {
    let ok = id
        .strip_prefix("op_")
        .map(|rest| {
            !rest.is_empty()
                && rest.len() <= 64
                && rest
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
        .unwrap_or(false);
    if !ok {
        bail!("invalid operation id {id:?} (expected op_ followed by 1-64 of [A-Za-z0-9_-])");
    }
    Ok(())
}

pub fn new_op_id() -> String {
    let now = chrono::Utc::now();
    let n = now.timestamp_nanos_opt().unwrap_or(0) ^ (std::process::id() as i64);
    let h = git2::Oid::hash_object(git2::ObjectType::Blob, n.to_string().as_bytes())
        .unwrap()
        .to_string();
    format!("op_{}_{}", now.format("%Y%m%d%H%M%S"), &h[..6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_ids_are_validated() {
        validate_op_id(&new_op_id()).unwrap();
        validate_op_id("op_x").unwrap();
        validate_op_id(&format!("op_{}", "a".repeat(64))).unwrap();
        for bad in [
            "",
            "op_",
            "op_../../etc",
            "../op_x",
            "op_a/b",
            "op_a b",
            "op_\u{e9}",
            "x_123",
            "op_a\0",
        ] {
            assert!(validate_op_id(bad).is_err(), "{bad:?} accepted");
        }
        assert!(validate_op_id(&format!("op_{}", "a".repeat(65))).is_err());
    }
}
