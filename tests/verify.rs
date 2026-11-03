//! Criteria V (ADR 0011): verify results produced outside Gitent.
mod common;
use common::*;

/// users repo with `feature` (cache) and `main` renamed; returns the feature commit.
fn diverged() -> (TestRepo, String) {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    (r, feature)
}

fn commit_all(r: &TestRepo, msg: &str) {
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", msg]);
}

#[test]
fn verify_good_and_bad_backports() {
    // A correct manual backport.
    let (r, feature) = diverged();
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "backport caching (by hand)");
    let (code, v) = r.gitent(&["verify", &feature]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["verified"], true);
    assert_eq!(v["base"], r.rev("HEAD~1").as_str());
    let checks = v["validation"]["checks"].to_string();
    assert!(
        checks.contains("\"requirements\"") && checks.contains("target_preserved"),
        "{checks}"
    );

    // Stale port: resurrects app/users.py, original tests pass against it.
    let (r, feature) = diverged();
    r.git(&[
        "checkout",
        &feature,
        "--",
        "app/users.py",
        "tests/test_cache.py",
    ]);
    commit_all(&r, "backport by copying the original files");
    let (code, v) = r.gitent(&["verify", &feature]);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("recreates path"),
        "{v}"
    );

    // Ported the test but not the behaviour.
    let (r, feature) = diverged();
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "half a backport");
    let (code, v) = r.gitent(&["verify", &feature]);
    assert_eq!(code, 2, "{v}");
    let failed: Vec<String> = v["validation"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["passed"] == false)
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        failed.contains(&"requirements".to_string()) && failed.contains(&"tests".to_string()),
        "{failed:?}"
    );
    let (_, text) = r.gitent_text(&["verify", &feature]);
    assert!(
        text.contains("✗ rejected") && text.contains("requirements"),
        "{text}"
    );
}

#[test]
fn verify_multi_commit_result() {
    let (r, feature) = diverged();
    let base = r.head();
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    commit_all(&r, "backport part 1: code");
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "backport part 2: tests");
    let (code, v) = r.gitent(&["verify", &feature, "--onto", &base]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["base"], base.as_str());
    // Part 1 alone does not carry the source's test.
    let (code, v) = r.gitent(&["verify", &feature, "HEAD~1"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["validation"].to_string().contains("source_tests_present"));
}

#[test]
fn verify_working_tree_is_side_effect_free() {
    let (r, feature) = diverged();
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    r.git(&["add", "app/accounts.py"]); // partially staged, on purpose
    let status = r.git(&["status", "--porcelain"]);
    let staged = r.git(&["diff", "--cached", "--stat"]);
    let head = r.head();
    let refs = r.git(&["for-each-ref"]);
    let (code, v) = r.gitent(&["verify", &feature, "--working-tree"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"], "working-tree");
    assert_eq!(
        r.git(&["status", "--porcelain"]),
        status,
        "working tree untouched"
    );
    assert_eq!(
        r.git(&["diff", "--cached", "--stat"]),
        staged,
        "index untouched"
    );
    assert_eq!(r.head(), head);
    assert_eq!(r.git(&["for-each-ref"]), refs, "no refs created");
    assert_eq!(r.worktrees(), 1, "temporary worktree removed");
    let (code, _) = r.gitent(&["verify", &feature, "--working-tree", "--record"]);
    assert_eq!(code, 1, "--record needs a commit");
}

#[test]
fn verify_revert_and_merge() {
    // Revert after refactor, done by hand.
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    r.commit("feat: cache user lookups", &[]);
    let cache = r.head();
    r.git(&["rm", "-q", "app/users.py"]);
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.write("tests/test_users.py", TEST_ACCOUNTS_PY);
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    r.commit("refactor: move users to accounts", &[]);
    let refactored = r.head();
    r.write("app/accounts.py", ACCOUNTS_PY);
    r.rm("tests/test_cache.py");
    commit_all(&r, "revert caching (by hand)");
    let (code, v) = r.gitent(&["verify", &cache, "--kind", "revert"]);
    assert_eq!(code, 0, "{v}");
    // A "revert" that leaves the behaviour in place.
    r.git(&["reset", "-q", "--hard", &refactored]);
    r.write("app/db.py", &(DB_PY.to_string() + "# reverted\n"));
    commit_all(&r, "pretend revert");
    let (code, v) = r.gitent(&["verify", &cache, "--kind", "revert"]);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("revert_effective"),
        "{v}"
    );

    // Merge, resolved by hand: both sides' requirements must hold.
    let r = TestRepo::empty();
    r.write("app/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write(
        "app/greet.py",
        "def greet(name):\n    return \"Hello, \" + name\n",
    );
    r.write(".gitignore", "__pycache__/\n");
    r.gitent(&["init"]);
    commit_all(&r, "base");
    r.git(&["checkout", "-qb", "punct"]);
    r.write(
        "app/greet.py",
        "def greet(name, punct=\"\"):\n    return \"Hello, \" + name + punct\n",
    );
    r.write("tests/test_punct.py", "import unittest\nfrom app.greet import greet\n\n\nclass P(unittest.TestCase):\n    def test_punct(self):\n        self.assertEqual(greet(\"Bob\", \"?\"), \"Hello, Bob?\")\n");
    r.commit("feat: punctuation", &[]);
    r.git(&["checkout", "-q", "main"]);
    r.write(
        "app/greet.py",
        "def greet(name):\n    return \"Hello, \" + name.strip().title()\n",
    );
    r.write("tests/test_title.py", "import unittest\nfrom app.greet import greet\n\n\nclass T(unittest.TestCase):\n    def test_title_case(self):\n        self.assertEqual(greet(\" ada lovelace \"), \"Hello, Ada Lovelace\")\n");
    r.commit("feat: title case", &[]);
    let ours = r.head();
    let (ok, _) = r.git_try(&["merge", "--no-edit", "punct"]);
    assert!(!ok, "conflict expected");
    r.write(
        "app/greet.py",
        "def greet(name, punct=\"\"):\n    return \"Hello, \" + name.strip().title() + punct\n",
    );
    commit_all(&r, "merge punct (resolved by hand)");
    let (code, v) = r.gitent(&["verify", "punct", "--kind", "merge"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["base"], ours.as_str());
    // A resolution that silently drops their side.
    r.git(&["reset", "-q", "--hard", &ours]);
    let _ = r.git_try(&["merge", "--no-edit", "punct"]);
    r.write(
        "app/greet.py",
        "def greet(name):\n    return \"Hello, \" + name.strip().title()\n",
    );
    commit_all(&r, "merge punct (kept ours)");
    let (code, v) = r.gitent(&["verify", "punct", "--kind", "merge"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"].as_str().unwrap().contains("punct"), "{v}");
}

#[test]
fn verify_record() {
    let (r, feature) = diverged();
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    commit_all(&r, "half a backport");
    let (code, v) = r.gitent(&["verify", &feature, "--record"]);
    assert_eq!(code, 2);
    assert_eq!(v["recorded"], false);
    assert!(
        !r.git_try(&["notes", "--ref", "gitent", "show", "HEAD"]).0,
        "no record on failure"
    );

    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.git(&["commit", "-qam", "finish backport"]);
    let (code, v) = r.gitent(&["verify", &feature, "--onto", "HEAD~2", "--record"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["recorded"], true);
    let note = r.note("HEAD");
    assert_eq!(note["provenance"]["operation"], "verify");
    assert_eq!(note["provenance"]["executor"], "external");
    assert_eq!(
        note["provenance"]["derived_from"]["commit"],
        feature.as_str()
    );
    assert_eq!(note["intent_id"], r.note(&feature)["intent_id"]);
    // The backport now participates in the ledger like any gitent realisation.
    let (code, chk) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{chk}");
    assert_eq!(chk["results"].as_array().unwrap().len(), 1);
    // Recording twice leaves the record alone.
    let (_, v) = r.gitent(&["verify", &feature, "--onto", "HEAD~2", "--record"]);
    assert_eq!(v["recorded"], false);
}

#[test]
fn tests_removed_by_the_source_change_may_disappear() {
    // Found on real Django backports: the original fix itself deleted a test;
    // a correct port also deletes it and must not be rejected for that.
    let r = TestRepo::users();
    r.git(&["checkout", "-qb", "fix"]);
    r.write("tests/test_users.py", &TEST_USERS_PY.replace("    def test_get_user(self):\n        self.assertEqual(get_user(1)[\"name\"], \"user1\")\n\n", ""));
    r.write("tests/test_names.py", "import unittest\nfrom app.users import get_user\n\n\nclass N(unittest.TestCase):\n    def test_user_name_format(self):\n        self.assertEqual(get_user(1)[\"name\"], \"user1\")\n");
    r.commit("test: move get_user test into test_names", &["--no-prove"]);
    let fix = r.head();
    r.git(&["checkout", "-q", "main"]);
    r.write("README.md", "x\n");
    commit_all(&r, "docs");
    let (ok, out) = r.git_try(&["cherry-pick", &fix]);
    assert!(ok, "{out}");
    let (code, v) = r.gitent(&["verify", &fix]);
    assert_eq!(code, 0, "{v}");
}
