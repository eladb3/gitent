//! Criteria A (foundation) and B (semantic commit).
mod common;
use common::*;

#[test]
fn init_creates_config_and_is_idempotent() {
    let r = TestRepo::empty();
    r.write("tests/test_x.py", "");
    let (code, v) = r.gitent(&["init", "--agent", "claude-code"]);
    assert_eq!(code, 0);
    assert_eq!(v["initialized"], true);
    let cfg = r.read(".gitent/config.toml");
    assert!(cfg.contains("claude -p"), "{cfg}");
    assert!(
        cfg.contains("python3 -m unittest -q -k {test} {module}"),
        "selector detected: {cfg}"
    );
    let (code, v) = r.gitent(&["init"]);
    assert_eq!(code, 0);
    assert_eq!(v["initialized"], false, "second init must not overwrite");
    // pytest projects get pytest (plain `def test_x()` functions are invisible to unittest).
    r.write("conftest.py", "");
    let (code, _) = r.gitent(&["init", "--force"]);
    assert_eq!(code, 0);
    let cfg = r.read(".gitent/config.toml");
    assert!(cfg.contains("python3 -m pytest -q {file} -k {test}"), "{cfg}");
    assert_eq!(r.read(".gitent/config.toml"), cfg);
    let (code, _) = r.gitent(&["init", "--agent", "nope"]);
    assert_eq!(code, 1);
}

#[test]
fn commit_records_intent_and_requirements() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let v = r.commit("feat: cache user lookups", &[]);
    let commit = v["commit"].as_str().unwrap().to_string();
    assert_eq!(commit, r.head());

    // A2: persisted in refs/notes/gitent and readable after the process exits.
    let note = r.note("HEAD");
    assert_eq!(note["schema"], "gitent/semantic-change@3");
    assert_eq!(note["kind"], "feature");
    assert_eq!(note["title"], "cache user lookups");
    assert_eq!(note["realization"]["commit"], commit.as_str());
    assert!(note["affected"]["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "get_user"));

    // R1 + R3: the added test is an auto requirement, proven fail-before / pass-after.
    let reqs = note["requirements"].as_array().unwrap();
    assert_eq!(reqs.len(), 1, "{reqs:?}");
    assert_eq!(reqs[0]["kind"], "test");
    assert_eq!(reqs[0]["test"], "test_cache_hit");
    assert_eq!(reqs[0]["origin"], "auto");
    assert_eq!(reqs[0]["proof"]["before"], "fail");
    assert_eq!(reqs[0]["proof"]["after"], "pass");

    let (code, log) = r.gitent(&["log"]);
    assert_eq!(code, 0);
    assert_eq!(log["entries"][0]["intent_id"], note["intent_id"]);
    assert_eq!(log["entries"][0]["requirements"], 1);
    assert!(
        log["entries"][1].get("intent_id").is_none(),
        "base commit has no record"
    );

    let (code, show) = r.gitent(&["show", note["intent_id"].as_str().unwrap()]);
    assert_eq!(code, 0);
    assert_eq!(show["commit"], commit.as_str());
}

#[test]
fn git_compat() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.commit("cache", &["--no-prove"]);
    let msg = r.git(&["log", "-1", "--format=%B"]);
    assert!(msg.contains("Gitent-Intent: int_"), "{msg}");
    let (ok, out) = r.git_try(&["fsck", "--no-dangling"]);
    assert!(ok, "{out}");
    assert_eq!(
        r.git(&["status", "--porcelain"]),
        "",
        "working tree clean after commit"
    );
    // The trailer is optional (the note links commit and record anyway).
    r.local_config("[commit]\nintent_trailer = false\n");
    r.write("app/db.py", &(DB_PY.to_string() + "\n# tweak\n"));
    r.commit("tweak", &["--no-prove"]);
    let msg = r.git(&["log", "-1", "--format=%B"]);
    assert!(!msg.contains("Gitent-Intent"), "{msg}");
    assert!(r.note("HEAD")["intent_id"]
        .as_str()
        .unwrap()
        .starts_with("int_"));
}

#[test]
fn json_output_and_schema() {
    let r = TestRepo::users();
    let (code, v) = r.gitent(&["schema"]);
    assert_eq!(code, 0);
    assert_eq!(v["$id"], "gitent/semantic-change@3");
    assert!(v["$defs"]["requirement"].is_object());
    let (code, v) = r.gitent(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(v["branch"], "main");
    assert_eq!(v["config"]["executor"], "host");
    let (code, v) = r.gitent(&["show", "does-not-exist"]);
    assert_eq!(code, 1);
    assert!(v["error"].as_str().unwrap().contains("unknown revision"));
}

#[test]
fn commit_heuristic_fallback_and_require_llm() {
    let r = TestRepo::users();
    let broken = r.script("broken_llm.sh", "cat > /dev/null; echo boom >&2; exit 3");
    r.write("app/users.py", USERS_CACHED_PY);
    let (code, v) = r.gitent(&[
        "--provider",
        "command",
        "commit",
        "-m",
        "fix: x",
        "--dry-run",
    ]);
    // provider=command without llm.command is a configuration error
    assert_eq!(code, 1, "{v}");
    let env = [("GITENT_LLM_COMMAND", broken.as_str())];
    let (code, v) = r.gitent_env(
        &[
            "--provider",
            "command",
            "commit",
            "-m",
            "fix: handle cache",
            "--require-llm",
        ],
        &env,
    );
    assert_eq!(code, 1);
    assert!(
        v["error"].as_str().unwrap().contains("LLM analysis failed"),
        "{v}"
    );
    assert_eq!(
        r.git(&["status", "--porcelain"]).lines().count(),
        1,
        "nothing committed"
    );
    let (code, v) = r.gitent_env(
        &["--provider", "command", "commit", "-m", "fix: handle cache"],
        &env,
    );
    assert_eq!(code, 0, "{v}");
    let rec = &v["record"];
    assert_eq!(rec["provenance"]["analysis"]["method"], "heuristic");
    assert_eq!(rec["kind"], "fix");
    assert!(rec["provenance"]["analysis"]["warnings"][0]
        .as_str()
        .unwrap()
        .contains("heuristic"));
}

#[test]
fn commit_with_llm_analyzer_claims_are_checked() {
    let r = TestRepo::users();
    let reply = serde_json::json!({
        "title": "Cache user lookups in memory",
        "kind": "perf",
        "intent": "Avoid repeated database loads for the same user.",
        "behavior": "get_user(uid) loads each uid from the db at most once per process.",
        "symbols": [{"name": "get_user"}, {"name": "phantom_symbol"}],
        "changes": [{"summary": "memoize get_user", "files": ["app/users.py", "app/imaginary.py"]}],
        "requirements": [
            {"statement": "A repeated lookup of the same id hits the database once", "test": "test_cache_hit"},
            {"statement": "Cache survives restarts", "test": "test_not_in_diff"},
            {"statement": "Lookups are fast", "check": {"run": "touch PWNED", "expect_exit": 0}}
        ],
        "constraints": ["get_user keeps its signature"],
        "confidence": 0.9
    });
    let llm = r.script(
        "llm.sh",
        &format!("cat > /dev/null\ncat <<'EOF'\nHere you go:\n```json\n{reply}\n```\nEOF"),
    );
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let (code, v) = r.gitent_env(
        &["--provider", "command", "commit", "-m", "cache"],
        &[("GITENT_LLM_COMMAND", &llm)],
    );
    assert_eq!(code, 0, "{v}");
    let rec = &v["record"];
    assert_eq!(rec["provenance"]["analysis"]["method"], "llm");
    assert_eq!(rec["kind"], "perf");
    assert_eq!(rec["title"], "Cache user lookups in memory");
    // B2: claims checked against the diff.
    let syms = rec["affected"]["symbols"].as_array().unwrap();
    assert_eq!(
        syms.iter().find(|s| s["name"] == "phantom_symbol").unwrap()["verified"],
        false
    );
    assert_eq!(
        rec["changes"][0]["files"],
        serde_json::json!(["app/users.py"])
    );
    // R2: requirement kinds are checked; LLM commands are never executed.
    let reqs = rec["requirements"].as_array().unwrap();
    let find = |s: &str| {
        reqs.iter()
            .find(|q| q["statement"].as_str().unwrap().starts_with(s))
            .unwrap_or_else(|| panic!("{s}: {reqs:?}"))
    };
    assert_eq!(find("A repeated lookup")["kind"], "test");
    assert_eq!(find("A repeated lookup")["proof"]["after"], "pass");
    assert_eq!(find("Cache survives")["kind"], "judged");
    assert_eq!(find("Lookups are fast")["kind"], "judged");
    assert!(find("Lookups are fast").get("check").is_none());
    assert_eq!(find("get_user keeps")["category"], "constraint");
    assert!(
        !r.path.join("PWNED").exists(),
        "LLM-proposed command must never run"
    );
}

#[test]
fn commit_with_host_intent() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    let intent = serde_json::json!({
        "title": "Memoize user lookups",
        "intent": "Reduce db load",
        "behavior": "same uid -> one db call",
        "requirements": [{"statement": "users module still imports", "check": {"run": "python3 -c 'import app.users'", "expect_exit": 0}}]
    });
    let f = r.tools.join("intent.json");
    std::fs::write(&f, intent.to_string()).unwrap();
    let (code, v) = r.gitent(&["commit", "--intent-file", f.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["record"]["provenance"]["analysis"]["method"], "host");
    assert_eq!(v["record"]["title"], "Memoize user lookups");
    let q = &v["record"]["requirements"][0];
    assert_eq!(q["kind"], "command");
    assert_eq!(q["origin"], "author");
    // passes both before and after: flagged as non-discriminating, not rejected (prove=warn)
    assert_eq!(q["proof"]["before"], "pass");
    let w = v["record"]["provenance"]["analysis"]["warnings"].to_string();
    assert!(w.contains("non_discriminating"), "{w}");
    let bad = r.tools.join("bad.json");
    std::fs::write(&bad, r#"{"title": ""}"#).unwrap();
    r.write("app/db.py", "CALLS = {}\n");
    let (code, _) = r.gitent(&["commit", "--intent-file", bad.to_str().unwrap()]);
    assert_eq!(code, 1, "empty title rejected");
}
