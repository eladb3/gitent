//! Criteria G1-G3 (ADR 0012): `gitent verify-pr` on PR-shaped histories.
mod common;
use common::*;

fn commit_all(r: &TestRepo, msg: &str) {
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", msg]);
}

/// users repo: `feature` = caching (the original change), `main` renamed; PR branch `backport` from main.
fn pr_setup() -> (TestRepo, String) {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    r.git(&["checkout", "-qb", "backport"]);
    (r, feature)
}

#[test]
fn cherry_pick_x_clean_backport_is_verified() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.write("README.md", "docs\n");
    commit_all(&r, "docs");
    r.git(&["checkout", "-qb", "backport"]);
    r.git(&["cherry-pick", "-x", &feature]);
    let md = r.tools.join("report.md");
    let (code, v) = r.gitent(&[
        "verify-pr",
        "--base",
        "main",
        "--markdown",
        md.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"], "verified");
    assert_eq!(v["detected"][0]["marker"], "cherry picked from");
    let report = std::fs::read_to_string(&md).unwrap();
    assert!(
        report.starts_with("<!-- gitent-verify-pr -->")
            && report.contains("✅ Gitent: backport verified"),
        "{report}"
    );
}

#[test]
fn manual_backport_with_follow_up_fix_is_verified_as_a_whole() {
    let (r, feature) = pr_setup();
    // First commit ports the code only (would fail alone), follow-up adds the test.
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    commit_all(
        &r,
        &format!("[stable] Cache user lookups\n\nBackport of {feature} from main."),
    );
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "port the test too");
    let (code, v) = r.gitent(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["verifications"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["verifications"][0]["result"],
        r.head().as_str(),
        "whole PR verified at its head"
    );
}

#[test]
fn stale_port_is_rejected_with_a_readable_report() {
    let (r, feature) = pr_setup();
    r.git(&[
        "checkout",
        &feature,
        "--",
        "app/users.py",
        "tests/test_cache.py",
    ]);
    commit_all(
        &r,
        &format!("Cache user lookups\n\n(cherry picked from commit {feature})"),
    );
    let (code, v) = r.gitent(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["result"], "rejected");
    let md = v["markdown"].as_str().unwrap();
    assert!(
        md.contains("❌ Gitent: backport rejected")
            && md.contains("✗ `target_preserved`")
            && md.contains("app/users.py"),
        "{md}"
    );
    // Human output is the markdown report.
    let (code, text) = r.gitent_text(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 2);
    assert!(text.contains("backport rejected"), "{text}");
}

#[test]
fn no_marker_is_neutral_unless_required() {
    let (r, _) = pr_setup();
    r.write("NOTES.md", "x\n");
    commit_all(&r, "unrelated change");
    let (code, v) = r.gitent(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"], "neutral");
    assert!(v["markdown"]
        .as_str()
        .unwrap()
        .contains("nothing to verify"));
    let (code, v) = r.gitent(&["verify-pr", "--base", "main", "--require-source"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["result"], "rejected");
}

#[test]
fn explicit_source_and_missing_source() {
    let (r, feature) = pr_setup();
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "port caching (no marker)");
    let (code, v) = r.gitent(&["verify-pr", "--base", "main", "--source", &feature]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["detected"][0]["marker"], "--source");
    // A marker pointing at a commit that is not in the clone.
    r.write("x.txt", "x\n");
    commit_all(
        &r,
        "more\n\n(cherry picked from commit 1234567890abcdef1234567890abcdef12345678)",
    );
    let (code, v) = r.gitent(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 1, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("fetch-depth: 0"),
        "{v}"
    );
}

#[test]
fn several_sources_are_verified_per_commit_and_reverts_detected() {
    let r = TestRepo::users();
    // Two independent changes on main...
    r.git(&["checkout", "-qb", "dev"]);
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    r.commit("feat: cache", &[]);
    let cache = r.head();
    r.write(
        "app/db.py",
        &(DB_PY.to_string() + "\n\ndef count():\n    return CALLS[\"n\"]\n"),
    );
    r.write("tests/test_count.py", "import unittest\nfrom app import db\n\n\nclass C(unittest.TestCase):\n    def test_count(self):\n        self.assertEqual(db.count(), db.CALLS[\"n\"])\n");
    r.commit("feat: db.count()", &[]);
    let count = r.head();
    // ...both cherry-picked to the release branch in one PR.
    r.git(&["checkout", "-q", "main"]);
    r.git(&["checkout", "-qb", "backport"]);
    r.git(&["cherry-pick", "-x", &cache]);
    r.git(&["cherry-pick", "-x", &count]);
    let (code, v) = r.gitent(&["verify-pr", "--base", "main"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        v["verifications"].as_array().unwrap().len(),
        2,
        "per commit"
    );

    // A plain `git revert` PR is detected as a revert.
    r.git(&["checkout", "-q", "dev"]);
    r.git(&["checkout", "-qb", "revert-pr"]);
    r.git(&["revert", "--no-edit", &count]);
    let (code, v) = r.gitent(&["verify-pr", "--base", "dev"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["detected"][0]["kind"], "revert");
}
