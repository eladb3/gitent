//! Bounded subprocess execution.

use anyhow::{Context, Result};
use std::cell::Cell;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcOutput {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

impl ProcOutput {
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }

    /// stdout+stderr, truncated from the front to keep the tail (where errors are).
    pub fn combined_tail(&self, max: usize) -> String {
        let s = format!("{}{}", self.stdout, self.stderr);
        tail(&s, max)
    }
}

pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(truncated)…\n{}", &s[start..])
}

/// How long to keep reading output after the main process ended (or was killed)
/// before giving up on descendants that still hold the pipes.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Which parts of gitent's own environment a child process inherits (ADR 0016 §1).
///
/// Neither policy passes the parent environment through: children start from an
/// empty environment and get an allowlist. Secrets (`*_TOKEN`, `*_KEY`,
/// `*_SECRET`, `*PASSWORD*`, `ACTIONS_*`, the `$GITHUB_ENV`-style workflow
/// command files, …) are never passed unless named in `GITENT_PASS_ENV`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Code from the repository: test commands, test selectors, requirement
    /// `check.run`, the builtin agent's `run` tool. Allowlist only.
    RepoCode,
    /// An LLM or coding-agent CLI (`llm.command`, `executor.command`): the
    /// allowlist plus the provider's own key and config variables
    /// (`ANTHROPIC_*`, `OPENAI_*`, `CLAUDE_*`, `CODEX_*`, `GEMINI_*`), but never
    /// GitHub tokens or the workflow command files.
    LlmCli,
}

/// Variables passed by name (toolchains, locale, terminal, CI marker, git identity).
const ALLOW_EXACT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    "CI",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "LD_LIBRARY_PATH",
    "PKG_CONFIG_PATH",
    "RUST_BACKTRACE",
    "GOPATH",
    "GOROOT",
    "GOCACHE",
    "GOMODCACHE",
    "GOFLAGS",
    "GOTOOLCHAIN",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "CONDA_DEFAULT_ENV",
    "JAVA_HOME",
    "GRADLE_USER_HOME",
    "MAVEN_HOME",
    "M2_HOME",
    "GEM_HOME",
    "GEM_PATH",
    "BUNDLE_PATH",
    "DOTNET_ROOT",
    "NVM_DIR",
    "PNPM_HOME",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    // Windows essentials.
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
];

/// Variables passed by prefix (secrets among them are still filtered out).
const ALLOW_PREFIX: &[&str] = &[
    "LC_",
    "XDG_",
    "PYTHON",
    "NODE_",
    "npm_config_",
    "NPM_CONFIG_",
    "CARGO_",
    "RUSTUP_",
];

/// Proxy settings: passed only when they carry no credentials (`user:pass@`).
const PROXY_VARS: &[&str] = &[
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
];

/// Provider key and config variables an LLM / agent CLI needs (`EnvPolicy::LlmCli`).
const LLM_PREFIX: &[&str] = &["ANTHROPIC_", "OPENAI_", "CLAUDE_", "CODEX_", "GEMINI_"];

/// GitHub Actions' workflow command files: writing to them changes later steps.
const WORKFLOW_FILES: &[&str] = &[
    "GITHUB_ENV",
    "GITHUB_PATH",
    "GITHUB_OUTPUT",
    "GITHUB_STATE",
    "GITHUB_STEP_SUMMARY",
];

/// Looks like a credential, or lets a child act on the CI job (never passed by policy).
pub fn is_secret_name(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    u.contains("TOKEN")
        || u.contains("SECRET")
        || u.contains("PASSWORD")
        || u.contains("PASSWD")
        || u.contains("CREDENTIAL")
        || u.contains("_AUTH")
        || u.ends_with("_KEY")
        || u.ends_with("APIKEY")
        || u == "KEY"
        || u.starts_with("ACTIONS_")
        || WORKFLOW_FILES.contains(&u.as_str())
}

/// GitHub credentials and job-control variables: denied even to LLM CLIs.
fn is_github_secret(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    (u.starts_with("GITHUB_") && is_secret_name(&u))
        || u.starts_with("GH_")
        || u.starts_with("ACTIONS_")
        || u == "GITENT_TOKEN"
        || WORKFLOW_FILES.contains(&u.as_str())
}

fn proxy_without_credentials(value: &str) -> bool {
    let authority = value.split_once("://").map(|(_, r)| r).unwrap_or(value);
    let authority = authority.split('/').next().unwrap_or("");
    !authority.contains('@')
}

/// Entries of `GITENT_PASS_ENV`: names, or prefixes ending in `*`.
fn pass_matches(pass: &str, name: &str) -> bool {
    pass.split(',')
        .map(str::trim)
        .any(|p| match p.strip_suffix('*') {
            Some(prefix) => !prefix.is_empty() && name.starts_with(prefix),
            None => !p.is_empty() && p == name,
        })
}

/// Whether `name=value` from gitent's environment reaches a child under `policy`.
/// `pass` is the value of `GITENT_PASS_ENV` (explicit consent overrides the filters).
pub fn passes(policy: EnvPolicy, name: &str, value: &str, pass: &str) -> bool {
    if pass_matches(pass, name) {
        return true;
    }
    if policy == EnvPolicy::LlmCli
        && LLM_PREFIX.iter().any(|p| name.starts_with(p))
        && !is_github_secret(name)
    {
        return true;
    }
    if is_secret_name(name) {
        return false;
    }
    if PROXY_VARS.contains(&name) {
        return proxy_without_credentials(value);
    }
    ALLOW_EXACT.contains(&name) || ALLOW_PREFIX.iter().any(|p| name.starts_with(p))
}

/// The part of `parent` a child inherits under `policy` (`pass` = `GITENT_PASS_ENV`).
pub fn child_env(
    policy: EnvPolicy,
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    pass: &str,
) -> Vec<(OsString, OsString)> {
    parent
        .into_iter()
        .filter(|(k, v)| match (k.to_str(), v.to_str()) {
            (Some(k), Some(v)) => passes(policy, k, v, pass),
            // A non-UTF-8 value cannot be inspected: never a proxy.
            (Some(k), None) => !PROXY_VARS.contains(&k) && passes(policy, k, "", pass),
            _ => false,
        })
        .collect()
}

/// Run repository code: `run_shell_with(EnvPolicy::RepoCode, …)`.
///
/// This is the safe default: test commands, selectors and requirement checks
/// go through here. Only LLM and agent CLIs call
/// `run_shell_with(EnvPolicy::LlmCli, …)`.
pub fn run_shell(
    cmd: &str,
    cwd: &Path,
    stdin: Option<&str>,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<ProcOutput> {
    run_shell_with(EnvPolicy::RepoCode, cmd, cwd, stdin, env, timeout)
}

/// Run `sh -c <cmd>` in `cwd`, feeding `stdin`, with a timeout. The child's
/// environment is the part of gitent's that `policy` allows, plus `env`
/// (gitent's own `GITENT_*` role variables, always set).
///
/// The command runs in its own process group (ADR 0014 §7). On timeout the
/// whole group is killed, so test runners' worker processes cannot outlive it.
/// Output is collected by reader threads; if descendants keep the pipes open
/// after the main process ended, the group is killed and whatever was read so
/// far is returned.
pub fn run_shell_with(
    policy: EnvPolicy,
    cmd: &str,
    cwd: &Path,
    stdin: Option<&str>,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<ProcOutput> {
    let start = Instant::now();
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(cwd);
    let pass = std::env::var("GITENT_PASS_ENV").unwrap_or_default();
    c.env_clear();
    c.envs(child_env(policy, std::env::vars_os(), &pass));
    c.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("failed to spawn `{cmd}`"))?;
    let pgid = child.id();
    if let Some(input) = stdin {
        let mut si = child.stdin.take().unwrap();
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(input.as_bytes());
        });
    }
    let out = Collector::spawn(child.stdout.take().unwrap());
    let err = Collector::spawn(child.stderr.take().unwrap());
    let (code, timed_out) = match child.wait_timeout(timeout)? {
        Some(st) => (st.code(), false),
        None => {
            kill_group(pgid);
            let _ = child.kill();
            let _ = child.wait();
            (None, true)
        }
    };
    // Descendants may still hold the pipes: give them a moment, then kill the group.
    let deadline = Instant::now() + DRAIN_GRACE;
    if !(out.wait_until(deadline) && err.wait_until(deadline)) {
        kill_group(pgid);
        let d2 = Instant::now() + DRAIN_GRACE;
        out.wait_until(d2);
        err.wait_until(d2);
    }
    Ok(ProcOutput {
        code,
        timed_out,
        stdout: out.take(),
        stderr: err.take(),
        duration_ms: start.elapsed().as_millis(),
    })
}

/// Kill every process in the group led by `pgid`.
fn kill_group(pgid: u32) {
    #[cfg(unix)]
    {
        if let Ok(p) = i32::try_from(pgid) {
            if p > 0 {
                // SAFETY: plain syscall; a negative pid addresses the process group.
                unsafe {
                    libc::kill(-p, libc::SIGKILL);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

/// Reads a pipe to the end on a thread, keeping what was read available even
/// if the thread never finishes.
struct Collector {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
    finished: Cell<bool>,
}

impl Collector {
    fn spawn<R: Read + Send + 'static>(mut r: R) -> Collector {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let b2 = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match r.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => b2.lock().unwrap().extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        });
        Collector {
            buf,
            done: rx,
            finished: Cell::new(false),
        }
    }

    /// True once the pipe reached EOF (waits at most until `deadline`).
    fn wait_until(&self, deadline: Instant) -> bool {
        if self.finished.get() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let ok = !matches!(
            self.done.recv_timeout(left),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        self.finished.set(ok);
        ok
    }

    fn take(&self) -> String {
        let b = std::mem::take(&mut *self.buf.lock().unwrap());
        String::from_utf8_lossy(&b).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_with_stdin_and_timeout() {
        let d = std::env::temp_dir();
        let o = run_shell(
            "cat; echo err >&2",
            &d,
            Some("hello"),
            &[],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(o.success());
        assert_eq!(o.stdout, "hello");
        assert_eq!(o.stderr.trim(), "err");
        let o = run_shell("sleep 5", &d, None, &[], Duration::from_millis(200)).unwrap();
        assert!(o.timed_out && !o.success());
    }

    #[test]
    fn timeout_kills_the_whole_process_group() {
        let d = std::env::temp_dir();
        let t = Instant::now();
        let o = run_shell("sleep 30 & sleep 30", &d, None, &[], Duration::from_secs(1)).unwrap();
        assert!(o.timed_out && !o.success());
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "took {:?}: a grandchild kept the pipes open",
            t.elapsed()
        );
    }

    #[test]
    fn background_grandchild_does_not_block_after_exit() {
        let d = std::env::temp_dir();
        let t = Instant::now();
        let o = run_shell(
            "sleep 30 & echo done",
            &d,
            None,
            &[],
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(o.success(), "{o:?}");
        assert_eq!(o.stdout.trim(), "done");
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "took {:?}",
            t.elapsed()
        );
    }

    fn env_of(policy: EnvPolicy, vars: &[(&str, &str)], pass: &str) -> Vec<String> {
        child_env(
            policy,
            vars.iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
            pass,
        )
        .into_iter()
        .map(|(k, _)| k.into_string().unwrap())
        .collect()
    }

    const PARENT: &[(&str, &str)] = &[
        ("PATH", "/usr/bin"),
        ("HOME", "/home/u"),
        ("LC_ALL", "C"),
        ("CARGO_HOME", "/c"),
        ("PYTHONPATH", "/p"),
        ("npm_config_cache", "/n"),
        ("CI", "true"),
        ("ANTHROPIC_API_KEY", "sk-a"),
        ("OPENAI_API_KEY", "sk-o"),
        ("CLAUDE_CONFIG_DIR", "/cc"),
        ("GITENT_TOKEN", "t"),
        ("GH_TOKEN", "g"),
        ("GITHUB_TOKEN", "g"),
        ("ACTIONS_RUNTIME_TOKEN", "a"),
        ("ACTIONS_ID_TOKEN_REQUEST_URL", "https://x"),
        ("GITHUB_ENV", "/w/env"),
        ("GITHUB_PATH", "/w/path"),
        ("GITHUB_OUTPUT", "/w/out"),
        ("GITHUB_STATE", "/w/state"),
        ("GITHUB_STEP_SUMMARY", "/w/sum"),
        ("CARGO_REGISTRY_TOKEN", "c"),
        ("DB_PASSWORD", "p"),
        ("AWS_SECRET_ACCESS_KEY", "s"),
        ("NPM_CONFIG__AUTH", "x"),
        ("RANDOM_VAR", "r"),
        ("HTTPS_PROXY", "http://proxy:3128"),
        ("HTTP_PROXY", "http://user:pw@proxy:3128"),
    ];

    const NEVER: &[&str] = &[
        "GITENT_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "ACTIONS_RUNTIME_TOKEN",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "GITHUB_ENV",
        "GITHUB_PATH",
        "GITHUB_OUTPUT",
        "GITHUB_STATE",
        "GITHUB_STEP_SUMMARY",
        "CARGO_REGISTRY_TOKEN",
        "DB_PASSWORD",
        "AWS_SECRET_ACCESS_KEY",
        "NPM_CONFIG__AUTH",
        "RANDOM_VAR",
        "HTTP_PROXY",
    ];

    #[test]
    fn repo_code_gets_the_allowlist_and_no_secrets() {
        let e = env_of(EnvPolicy::RepoCode, PARENT, "");
        for k in [
            "PATH",
            "HOME",
            "LC_ALL",
            "CARGO_HOME",
            "PYTHONPATH",
            "npm_config_cache",
            "CI",
            "HTTPS_PROXY",
        ] {
            assert!(e.contains(&k.to_string()), "{k} missing: {e:?}");
        }
        for k in NEVER
            .iter()
            .chain(&["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "CLAUDE_CONFIG_DIR"])
        {
            assert!(!e.contains(&k.to_string()), "{k} leaked: {e:?}");
        }
    }

    #[test]
    fn llm_clis_get_provider_keys_but_no_github_tokens() {
        let e = env_of(EnvPolicy::LlmCli, PARENT, "");
        for k in [
            "PATH",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE_CONFIG_DIR",
        ] {
            assert!(e.contains(&k.to_string()), "{k} missing: {e:?}");
        }
        for k in NEVER {
            assert!(!e.contains(&k.to_string()), "{k} leaked: {e:?}");
        }
    }

    #[test]
    fn gitent_pass_env_is_explicit_consent() {
        let e = env_of(
            EnvPolicy::RepoCode,
            PARENT,
            " DB_PASSWORD , AWS_*,RANDOM_VAR,",
        );
        for k in ["DB_PASSWORD", "AWS_SECRET_ACCESS_KEY", "RANDOM_VAR"] {
            assert!(e.contains(&k.to_string()), "{k} missing: {e:?}");
        }
        assert!(!e.contains(&"GH_TOKEN".to_string()));
        // A bare `*` is not a wildcard for everything.
        let e = env_of(EnvPolicy::RepoCode, PARENT, "*");
        assert!(!e.contains(&"GH_TOKEN".to_string()));
    }

    #[test]
    fn child_does_not_see_a_planted_secret() {
        // Unique names: the process environment is shared with parallel tests.
        std::env::set_var("GITENT_PROC_TEST_PLANTED_TOKEN", "planted-secret");
        std::env::set_var("GITENT_PROC_TEST_UNLISTED", "unlisted");
        let d = std::env::temp_dir();
        for policy in [EnvPolicy::RepoCode, EnvPolicy::LlmCli] {
            let o = run_shell_with(
                policy,
                "env",
                &d,
                None,
                &[("GITENT_ROLE", "test".into())],
                Duration::from_secs(5),
            )
            .unwrap();
            assert!(o.success(), "{o:?}");
            assert!(!o.stdout.contains("planted-secret"), "{}", o.stdout);
            assert!(!o.stdout.contains("GITENT_PROC_TEST_UNLISTED"));
            assert!(o.stdout.lines().any(|l| l.starts_with("PATH=")));
            assert!(o.stdout.lines().any(|l| l == "GITENT_ROLE=test"));
        }
        std::env::remove_var("GITENT_PROC_TEST_PLANTED_TOKEN");
        std::env::remove_var("GITENT_PROC_TEST_UNLISTED");
    }

    #[test]
    fn tail_keeps_end() {
        assert_eq!(tail("abcdef", 3), "…(truncated)…\ndef");
    }
}
