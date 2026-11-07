//! Criteria S12-S14: trust boundaries round 2 (ADR 0016): child environments,
//! and direnv-style trust for the shared config's program-selecting keys.
mod common;
use common::*;

/// S12: repository code (a requirement check) and LLM CLIs get an allowlisted
/// environment: no GitHub tokens or workflow files; the LLM CLI gets its key.
#[test]
fn child_processes_get_an_allowlisted_environment() {
    let r = TestRepo::users();
    let check_env = r.tools.join("check.env");
    let llm_env = r.tools.join("llm.env");
    let llm = r.script(
        "llm.sh",
        &format!("cat > /dev/null\nenv > {}\necho '{{}}'", llm_env.display()),
    );
    r.write(
        "app/db.py",
        &DB_PY.replace("CALLS = {\"n\": 0}", "CALLS = {\"n\": 0}\nLIMIT = 1"),
    );
    let check = format!(
        "env > {} && python3 -c 'import app.db as d; assert d.LIMIT == 1'",
        check_env.display()
    );
    let (code, v) = r.gitent_env(
        &[
            "--provider",
            "command",
            "commit",
            "-m",
            "add LIMIT",
            "--require",
            "LIMIT is 1",
            "--check",
            &check,
        ],
        &[
            ("GITENT_LLM_COMMAND", &llm),
            ("GH_TOKEN", "gh-secret"),
            ("GITHUB_TOKEN", "github-secret"),
            ("GITHUB_ENV", "/tmp/github-env-file"),
            ("ACTIONS_RUNTIME_TOKEN", "actions-secret"),
            ("ANTHROPIC_API_KEY", "anthropic-key"),
            ("MY_SERVICE_PASSWORD", "pw-secret"),
            ("GITENT_PASS_ENV", "MY_PASSED_VAR"),
            ("MY_PASSED_VAR", "passed-value"),
        ],
    );
    assert_eq!(code, 0, "{v}");
    let check_seen = std::fs::read_to_string(&check_env).expect("check ran");
    let llm_seen = std::fs::read_to_string(&llm_env).expect("llm ran");
    for secret in [
        "gh-secret",
        "github-secret",
        "github-env-file",
        "actions-secret",
        "pw-secret",
    ] {
        assert!(!check_seen.contains(secret), "check saw {secret}");
        assert!(!llm_seen.contains(secret), "llm saw {secret}");
    }
    assert!(
        !check_seen.contains("anthropic-key"),
        "repo code saw the API key"
    );
    assert!(
        llm_seen.contains("ANTHROPIC_API_KEY=anthropic-key"),
        "the LLM CLI needs its key"
    );
    for seen in [&check_seen, &llm_seen] {
        assert!(seen.lines().any(|l| l.starts_with("PATH=")));
        assert!(
            seen.contains("MY_PASSED_VAR=passed-value"),
            "GITENT_PASS_ENV"
        );
    }
}

fn agent_config(cmd: &str) -> String {
    format!("[executor]\nkind = \"command\"\ncommand = \"{cmd}\"\n")
}

/// S13: a shared config's executor.command is ignored (with a warning) until
/// `gitent trust`; editing the file needs trusting again.
#[test]
fn untrusted_shared_executor_command_is_not_run() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    let marker = r.tools.join("agent-ran");
    let agent = r.script(
        "agent.sh",
        &format!("cat > /dev/null\ntouch {}\necho ok", marker.display()),
    );
    let set_config = |r: &TestRepo, text: &str| {
        r.write(".gitent/config.toml", text);
        r.git(&["commit", "-qam", "config"]);
    };
    // As if cloned: the malicious config is committed.
    set_config(&r, &agent_config(&agent));
    let attempt = |r: &TestRepo| {
        let o = r
            .gitent_cmd()
            .args(["--json", "cherry-pick", &feature, "--no-git"])
            .output()
            .unwrap();
        let _ = r.gitent(&["abort"]);
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    };

    // A cloned repository's config is not trusted; read-only commands still work.
    let (code, v) = r.gitent(&["status"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["config"]["executor_command"], "");
    let err = attempt(&r);
    assert!(!marker.exists(), "untrusted executor.command ran");
    assert!(
        err.contains("requires executor.command") && err.contains("gitent trust"),
        "{err}"
    );

    // Trusted, then edited: needs trust again.
    let (code, v) = r.gitent(&["trust"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["sets"]["executor.command"], agent.as_str());
    let (_, v) = r.gitent(&["status"]);
    assert_eq!(v["config"]["executor_command"], agent.as_str());
    set_config(&r, &format!("# edited\n{}", agent_config(&agent)));
    let err = attempt(&r);
    assert!(!marker.exists(), "edited config ran without new trust");
    assert!(err.contains("requires executor.command"), "{err}");

    // Trusted again: it runs.
    let (code, _) = r.gitent(&["trust"]);
    assert_eq!(code, 0);
    let out = attempt(&r);
    assert!(
        marker.exists(),
        "trusted executor.command did not run: {out}"
    );
}

/// S13: `gitent init` trusts the config it writes; the test command needs no trust.
#[test]
fn init_trusts_what_it_writes_and_test_command_needs_no_trust() {
    let r = TestRepo::empty();
    let (code, v) = r.gitent(&["init", "--agent", "claude-code"]);
    assert_eq!(code, 0, "{v}");
    let (_, v) = r.gitent(&["status"]);
    assert!(
        v["config"]["executor_command"]
            .as_str()
            .unwrap()
            .starts_with("claude"),
        "{v}"
    );
    r.write(
        ".gitent/config.toml",
        "[validation]\ntest_command = \"make test\"\n",
    );
    let (_, v) = r.gitent(&["status"]);
    assert_eq!(v["config"]["test_command"], "make test");
}
