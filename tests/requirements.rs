//! Criteria R (requirements): proof, author requirements, check, retire, ledger.
mod common;
use common::*;

#[test]
fn proof_rejects_requirement_failing_after_change() {
    let r = TestRepo::users();
    let before = r.head();
    // The "cache" change is missing, so the new test fails on the new commit.
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let (code, v) = r.gitent(&["commit", "-m", "add cache test"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["state"], "failed");
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("fails on the new commit"),
        "{v}"
    );
    assert_eq!(r.head(), before, "branch must not move");
    // --allow-failing records it instead
    let (code, v) = r.gitent(&[
        "commit",
        "-m",
        "add pending cache test",
        "--allow-failing",
        "test_cache_hit",
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["record"]["requirements"][0]["proof"]["after"], "fail");
}

#[test]
fn proof_flags_or_rejects_non_discriminating_checks() {
    let r = TestRepo::users();
    // A new test that already passes on the parent: it does not test this change.
    let t = "import unittest\nfrom app.users import get_user\n\n\nclass T(unittest.TestCase):\n    def test_name_format(self):\n        self.assertEqual(get_user(5)[\"name\"], \"user5\")\n";
    r.write("tests/test_format.py", t);
    r.write("app/users.py", USERS_CACHED_PY);
    let (code, v) = r.gitent(&["commit", "-m", "cache", "--dry-run"]);
    assert_eq!(code, 0);
    assert!(
        v["record"]["requirements"][0].get("proof").is_none(),
        "dry runs do not prove"
    );
    r.local_config("[requirements]\nprove = \"require\"\n");
    let (code, v) = r.gitent(&["commit", "-m", "cache"]);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("non_discriminating"),
        "{v}"
    );
    r.local_config("[requirements]\nprove = \"warn\"\n");
    let v = r.commit("cache", &[]);
    assert_eq!(v["record"]["requirements"][0]["proof"]["before"], "pass");
    assert!(v["record"]["provenance"]["analysis"]["warnings"]
        .to_string()
        .contains("non_discriminating"));
}

#[test]
fn author_requirements_from_flags() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let v = r.commit(
        "cache",
        &[
            "--require",
            "Repeated lookups hit the db once",
            "--test",
            "test_cache_hit",
            "--require",
            "The cache is a module-level dict",
            "--check",
            "grep -q '_CACHE = {}' app/users.py",
            "--require",
            "Memory use stays bounded",
        ],
    );
    let reqs = v["record"]["requirements"].as_array().unwrap();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    let by = |s: &str| reqs.iter().find(|q| q["statement"] == s).unwrap();
    assert_eq!(
        by("Repeated lookups hit the db once")["kind"],
        "test",
        "--test refines the auto requirement"
    );
    assert_eq!(by("Repeated lookups hit the db once")["origin"], "author");
    let c = by("The cache is a module-level dict");
    assert_eq!(c["kind"], "command");
    assert_eq!(c["proof"]["before"], "fail");
    assert_eq!(c["proof"]["after"], "pass");
    assert_eq!(by("Memory use stays bounded")["kind"], "judged");
}

#[test]
fn check_reports_regression_and_names_intention() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    r.commit("feat: cache user lookups", &[]);
    let (code, v) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["counts"]["pass"], 1);
    // Someone removes the cache with plain git.
    r.write("app/users.py", USERS_PY);
    r.git(&["commit", "-qam", "simplify users"]);
    let (code, v) = r.gitent(&["check"]);
    assert_eq!(code, 2, "{v}");
    let f = &v["results"][0];
    assert_eq!(f["status"], "fail");
    assert_eq!(f["intent_title"], "cache user lookups");
    // checking an older revision uses a temporary worktree and passes
    let (code, v) = r.gitent(&["check", "HEAD~1"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(r.worktrees(), 1, "temporary worktree removed");
    let (_, text) = r.gitent_text(&["check"]);
    assert!(
        text.contains("✗ req_") && text.contains("from \"cache user lookups\""),
        "{text}"
    );
}

#[test]
fn retire_and_check_ledger() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let v = r.commit("feat: cache user lookups", &[]);
    let id = v["record"]["requirements"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Breaking a live requirement with --check-ledger is rejected...
    r.write("app/users.py", USERS_PY);
    let (code, v) = r.gitent(&["commit", "-m", "drop cache", "--check-ledger", "--no-prove"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"].as_str().unwrap().contains(&id));
    // ...and so is weakening the test that backs it (the guard against "passing" by editing the check).
    r.write(
        "tests/test_cache.py",
        &TEST_CACHE_PY.replace(
            "assertEqual(db.CALLS[\"n\"] - before, 1)",
            "assertEqual(db.CALLS[\"n\"] - before, 2)",
        ),
    );
    let (code, v) = r.gitent(&["commit", "-m", "drop cache", "--check-ledger", "--no-prove"]);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("modifies_bound_test"),
        "{v}"
    );
    // ...unknown ids and missing reasons are errors...
    let (code, _) = r.gitent(&[
        "commit",
        "-m",
        "drop cache",
        "--retire",
        "req_00000000",
        "why",
    ]);
    assert_eq!(code, 1);
    let (code, _) = r.gitent(&["commit", "-m", "drop cache", "--retire", &id, " "]);
    assert_eq!(code, 1);
    // ...retiring with a reason is accepted and recorded.
    let v = r.commit(
        "drop cache",
        &[
            "--retire",
            &id,
            "caching moved to the db layer",
            "--check-ledger",
            "--no-prove",
        ],
    );
    assert_eq!(v["record"]["retires"][0]["id"], id.as_str());
    let (code, v) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v["results"].as_array().unwrap().len(),
        0,
        "retired requirement left the ledger"
    );
    let (_, log) = r.gitent(&["log", "-n", "1"]);
    assert_eq!(
        log["entries"][0]["retires"][0]["reason"],
        "caching moved to the db layer"
    );
}

#[test]
fn ledger_newest_event_wins_and_merges_keep_retirements() {
    // ADR 0013 §9: retire, then re-apply → live again.
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    let v = r.commit("feat: cache user lookups", &[]);
    let feat = r.head();
    let id = v["record"]["requirements"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    r.write("app/users.py", USERS_PY);
    r.rm("tests/test_cache.py");
    r.commit(
        "drop cache",
        &["--retire", &id, "caching moved", "--no-prove"],
    );
    let dropped = r.head();
    let (_, v) = r.gitent(&["check"]);
    assert_eq!(v["results"].as_array().unwrap().len(), 0, "{v}");

    // A side branch from before the retirement keeps the requirement; merging it must
    // not bring the retired requirement back.
    r.git(&["checkout", "-q", "-b", "side", &feat]);
    r.write("NOTES.md", "side\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "side work"]);
    let side = r.head();
    r.git(&["checkout", "-q", "main"]);
    let (code, v) = r.gitent(&["merge", "side"]);
    assert_eq!(code, 0, "{v}");
    let reqs = v["steps"][0]["requirements"].to_string();
    assert!(
        reqs.starts_with('['),
        "merge step lists its requirements: {v}"
    );
    assert!(!reqs.contains(&id), "retired on main stays retired: {reqs}");
    let _ = (dropped, side);

    // Re-applying the change makes the requirement live again (newest event wins).
    r.git(&["cherry-pick", "-x", &feat]);
    let (code, v) = r.gitent(&["verify", &feat, "--record"]);
    assert_eq!(code, 0, "{v}");
    let (code, v) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == id.as_str()),
        "re-applied requirement is live again: {v}"
    );
}

#[test]
fn a_selector_that_runs_zero_tests_does_not_prove_a_requirement() {
    // ADR 0013 §4: pytest-style functions under a unittest selector run 0 tests.
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write(
        "tests/test_plain.py",
        "from app import users\n\n\ndef test_plain_cache():\n    assert users.get_user(1) == users.get_user(1)\n",
    );
    let (code, v) = r.gitent(&["commit", "-m", "cache (pytest-style test)"]);
    assert_ne!(code, 0, "a vacuous requirement must not pass its proof: {v}");
    assert!(v.to_string().contains("ran 0 tests") || v.to_string().contains("NO TESTS RAN"), "{v}");
}
