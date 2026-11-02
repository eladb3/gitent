//! Configuration: `.gitent/config.toml` (shared) overlaid by
//! `.git/gitent/config.toml` (local), then environment variables, then CLI flags.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LlmConfig {
    /// anthropic | openai | command | scripted | none
    pub provider: String,
    pub model: String,
    pub base_url: String,
    /// Shell command for the `command` provider (prompt on stdin, reply on stdout).
    pub command: String,
    /// Path to a JSON array of canned responses for the `scripted` provider.
    pub script: String,
    pub timeout_secs: u64,
    pub max_tokens: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            provider: "none".into(),
            model: String::new(),
            base_url: String::new(),
            command: String::new(),
            script: String::new(),
            timeout_secs: 300,
            max_tokens: 16000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ExecutorConfig {
    /// builtin | command | host
    pub kind: String,
    pub command: String,
    pub timeout_secs: u64,
    /// Builtin agent: maximum tool steps per attempt.
    pub max_steps: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        ExecutorConfig {
            kind: "host".into(),
            command: String::new(),
            timeout_secs: 900,
            max_steps: 40,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ValidationConfig {
    pub test_command: String,
    /// Runs one test by name; `{test}` is replaced (e.g. `python3 -m pytest -q -k {test}`).
    pub test_selector: String,
    pub timeout_secs: u64,
    pub max_attempts: u32,
    /// Run the LLM verifier when a provider is configured.
    pub verifier: bool,
    pub require_verifier: bool,
    pub verifier_threshold: f64,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        ValidationConfig {
            test_command: String::new(),
            test_selector: String::new(),
            timeout_secs: 600,
            max_attempts: 2,
            verifier: true,
            require_verifier: false,
            verifier_threshold: 0.7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Config {
    pub llm: LlmConfig,
    pub executor: ExecutorConfig,
    pub validation: ValidationConfig,
    pub requirements: RequirementsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RequirementsConfig {
    /// warn | require | off
    pub prove: String,
    pub check_ledger_on_commit: bool,
    pub judge_in_operations: bool,
}

impl Default for RequirementsConfig {
    fn default() -> Self {
        RequirementsConfig {
            prove: "warn".into(),
            check_ledger_on_commit: false,
            judge_in_operations: true,
        }
    }
}

impl Config {
    /// Load config for a repository rooted at `workdir` with git dir `git_dir`.
    pub fn load(workdir: &Path, git_dir: &Path) -> Result<Config> {
        let mut merged = toml::Table::new();
        for p in [
            workdir.join(".gitent/config.toml"),
            git_dir.join("gitent/config.toml"),
        ] {
            if p.exists() {
                let text = std::fs::read_to_string(&p)?;
                let t: toml::Table =
                    toml::from_str(&text).with_context(|| format!("parsing {}", p.display()))?;
                merge(&mut merged, t);
            }
        }
        let mut cfg: Config = toml::Value::Table(merged)
            .try_into()
            .context("invalid gitent config")?;
        cfg.apply_env();
        Ok(cfg)
    }

    pub fn apply_env(&mut self) {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        if let Some(v) = env("GITENT_PROVIDER") {
            self.llm.provider = v;
        }
        if let Some(v) = env("GITENT_LLM_COMMAND") {
            self.llm.command = v;
        }
        if let Some(v) = env("GITENT_MODEL") {
            self.llm.model = v;
        }
        if let Some(v) = env("GITENT_EXECUTOR") {
            self.executor.kind = v;
        }
        if let Some(v) = env("GITENT_EXECUTOR_COMMAND") {
            self.executor.command = v;
        }
        if let Some(v) = env("GITENT_TEST_COMMAND") {
            self.validation.test_command = v;
        }
        if let Some(v) = env("GITENT_TEST_SELECTOR") {
            self.validation.test_selector = v;
        }
    }

    /// A preset for `gitent init --agent <name>`.
    pub fn preset(name: &str) -> Option<Config> {
        let mut c = Config::default();
        match name {
            "claude-code" | "claude" => {
                c.llm.provider = "command".into();
                c.llm.command = "claude -p".into();
                c.executor.kind = "command".into();
                c.executor.command = "claude -p --permission-mode acceptEdits".into();
            }
            "codex" => {
                c.llm.provider = "command".into();
                c.llm.command = "codex exec -".into();
                c.executor.kind = "command".into();
                c.executor.command = "codex exec --full-auto -".into();
            }
            "kiro" => {
                c.executor.kind = "command".into();
                c.executor.command =
                    "kiro-cli chat --no-interactive --trust-all-tools \"Follow the task in {brief}\"".into();
            }
            "gemini" => {
                c.executor.kind = "command".into();
                c.executor.command = "gemini --yolo -p \"Follow the task in {brief}\"".into();
            }
            "aider" => {
                c.executor.kind = "command".into();
                c.executor.command =
                    "aider --yes-always --no-auto-commits --message-file {brief}".into();
            }
            "builtin" | "anthropic" => {
                c.llm.provider = "anthropic".into();
                c.llm.model = "claude-opus-5-5".into();
                c.executor.kind = "builtin".into();
            }
            "openai" => {
                c.llm.provider = "openai".into();
                c.executor.kind = "builtin".into();
            }
            "host" | "none" => {}
            _ => return None,
        }
        Some(c)
    }

    pub const PRESETS: &'static [&'static str] = &[
        "claude-code",
        "codex",
        "kiro",
        "gemini",
        "aider",
        "builtin",
        "openai",
        "host",
    ];

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

fn merge(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_overrides_shared() {
        let d = tempfile::tempdir().unwrap();
        let wd = d.path();
        let gd = wd.join(".git");
        std::fs::create_dir_all(wd.join(".gitent")).unwrap();
        std::fs::create_dir_all(gd.join("gitent")).unwrap();
        std::fs::write(
            wd.join(".gitent/config.toml"),
            "[validation]\ntest_command='make test'\n[executor]\nkind='command'\ncommand='a'\n",
        )
        .unwrap();
        std::fs::write(gd.join("gitent/config.toml"), "[executor]\ncommand='b'\n").unwrap();
        let c = Config::load(wd, &gd).unwrap();
        assert_eq!(c.validation.test_command, "make test");
        assert_eq!(c.executor.kind, "command");
        assert_eq!(c.executor.command, "b");
        assert_eq!(c.validation.max_attempts, 2);
    }

    #[test]
    fn presets_roundtrip() {
        for p in Config::PRESETS {
            let c = Config::preset(p).unwrap();
            let back: Config = toml::from_str(&c.to_toml()).unwrap();
            assert_eq!(back, c);
        }
    }
}
