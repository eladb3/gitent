//! Criteria D (merge, rebase, revert) and R7/R8.
mod common;
use common::*;
use serde_json::Value;

const GREET_PY: &str = "def greet(name):\n    return \"Hello, \" + name\n";
const TEST_GREET_PY: &str = "import unittest\nfrom app.greet import greet\n\n\nclass GreetTest(unittest.TestCase):\n    def test_greet(self):\n        self.assertEqual(greet(\"Bob\"), \"Hello, Bob\")\n";
const OURS_PY: &str = "def greet(name):\n    return \"Hello, \" + name.strip().title()\n";
const TEST_TITLE_PY: &str = "import unittest\nfrom app.greet import greet\n\n\nclass TitleTest(unittest.TestCase):\n    def test_title_case(self):\n        self.assertEqual(greet(\" ada lovelace \"), \"Hello, Ada Lovelace\")\n";
const THEIRS_PY: &str = "def greet(name, punct=\"\"):\n    return \"Hello, \" + name + punct\n";
const TEST_PUNCT_PY: &str = "import unittest\nfrom app.greet import greet\n\n\nclass PunctTest(unittest.TestCase):\n    def test_punct(self):\n        self.assertEqual(greet(\"Bob\", \"?\"), \"Hello, Bob?\")\n";
const BOTH_PY: &str =
    "def greet(name, punct=\"\"):\n    return \"Hello, \" + name.strip().title() + punct\n";

/// main: title-casing; branch `punct`: punctuation parameter. Both edit the same line.
fn greet_repo() -> TestRepo {
    let r = TestRepo::empty();
    r.write("app/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write("app/greet.py", GREET_PY);
    r.write("tests/test_greet.py", TEST_GREET_PY);
    r.write(".gitignore", "__pycache__/\n");
    r.gitent(&["init"]);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "base: greet"]);
    r.git(&["checkout", "-qb", "punct"]);
    r.write("app/greet.py", THEIRS_PY);
    r.write("tests/test_punct.py", TEST_PUNCT_PY);
    r.commit("feat: optional punctuation in greetings", &[]);
    r.git(&["checkout", "-q", "main"]);
    r.write("app/greet.py", OURS_PY);
    r.write("tests/test_title.py", TEST_TITLE_PY);
    r.commit("feat: title-case and trim names", &[]);
    r
}

fn merge(r: &TestRepo, branch: &str, agent: &str, extra: &[&str]) -> (i32, Value) {
    let mut a = vec![
        "--executor",
        "command",
        "--executor-command",
        agent,
        "merge",
        branch,
    ];
    a.extend_from_slice(extra);
    r.gitent(&a)
}

#[test]
fn merge_clean_is_deterministic() {
    let r = greet_repo();
    r.git(&["checkout", "-qb", "farewell", "main~1"]);
    r.write(
        "app/farewell.py",
        "def bye(name):\n    return \"Bye, \" + name\n",
    );
    r.write("tests/test_farewell.py", "import unittest\nfrom app.farewell import bye\n\n\nclass ByeTest(unittest.TestCase):\n    def test_bye(self):\n        self.assertEqual(bye(\"Al\"), \"Bye, Al\")\n");
    r.commit("feat: farewells", &[]);
    r.git(&["checkout", "-q", "main"]);
    let ours = r.head();
    let (code, v) = merge(&r, "farewell", "false", &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "succeeded");
    assert_eq!(
        r.git(&["rev-list", "--parents", "-n", "1", "HEAD"])
            .split(' ')
            .count(),
        3,
        "two parents"
    );
    assert_eq!(r.rev("HEAD^1"), ours);
    let rec = r.note("HEAD");
    assert_eq!(rec["kind"], "merge");
    assert_eq!(rec["provenance"]["executor"], "git");
    // Ledger after the merge holds requirements from both sides.
    let (code, chk) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{chk}");
    let tests: Vec<&str> = chk["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["statement"].as_str().unwrap())
        .collect();
    assert!(
        tests.contains(&"bye") && tests.contains(&"title case"),
        "{tests:?}"
    );
}

#[test]
fn merge_conflict_composes_both_intentions() {
    let r = greet_repo();
    let theirs = r.rev("punct");
    let (ok, _) = r.git_try(&["merge", "--no-edit", "punct"]);
    assert!(!ok, "git merge conflicts");
    r.git(&["merge", "--abort"]);
    let agent = r.script(
        "merge_agent.sh",
        &format!("cat > /dev/null\n{}", heredoc("app/greet.py", BOTH_PY)),
    );
    let (code, v) = merge(&r, "punct", &agent, &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(r.rev("HEAD^2"), theirs);
    let rec = r.note("HEAD");
    assert_eq!(rec["kind"], "merge");
    assert_eq!(rec["provenance"]["composes"].as_array().unwrap().len(), 2);
    let (code, chk) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{chk}");
    assert_eq!(
        chk["counts"]["pass"], 2,
        "both sides' requirements live and passing"
    );

    // An agent that keeps only one side fails the other side's requirement.
    let r2 = greet_repo();
    let agent = r2.script(
        "one_side.sh",
        &format!("cat > /dev/null\n{}", heredoc("app/greet.py", OURS_PY)),
    );
    let (code, v) = merge(&r2, "punct", &agent, &["--max-attempts", "1"]);
    assert_eq!(code, 2, "{v}");
    let detail = v["steps"][0]["attempts"][1]["validation"].to_string();
    assert!(
        detail.contains("punct"),
        "names the broken requirement: {detail}"
    );
}

#[test]
fn merge_incompatible_intentions_stop() {
    let r = greet_repo();
    let llm = r.script(
        "llm.sh",
        r#"in=$(cat)
case "$in" in
  *"merge analyst"*) echo '{"compatible": false, "confidence": 0.95, "conflicts": [{"reason": "title-casing contradicts verbatim names"}]}' ;;
  *) echo '{"satisfied": true, "confidence": 0.9, "concerns": []}' ;;
esac"#,
    );
    let marker = r.tools.join("agent_called");
    let agent = r.script("agent.sh", &format!("touch {}", marker.display()));
    let env = [
        ("GITENT_PROVIDER", "command"),
        ("GITENT_LLM_COMMAND", llm.as_str()),
    ];
    let mut args = vec![
        "--executor",
        "command",
        "--executor-command",
        agent.as_str(),
        "merge",
        "punct",
    ];
    let (code, v) = r.gitent_env(&args, &env);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"].as_str().unwrap().contains("incompatible"), "{v}");
    assert!(
        !marker.exists(),
        "no agent effort spent on an incompatible merge"
    );
    r.gitent(&["abort"]);
    args.push("--force");
    let (_, v) = r.gitent_env(&args, &env);
    assert!(marker.exists(), "--force proceeds to the agent: {v}");
}

#[test]
fn rebase_replays_intentions_and_failure_leaves_branch() {
    // Clean: feature rebased onto main with an unrelated change.
    let r = TestRepo::users();
    r.add_cache_feature();
    r.write("README.md", "docs\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "docs"]);
    let main = r.head();
    r.git(&["checkout", "-q", "feature"]);
    let (code, v) = r.gitent(&["rebase", "main"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(r.rev("HEAD~1"), main);
    assert_eq!(r.note("HEAD")["provenance"]["operation"], "rebase");
    let (code, _) = r.gitent(&["check"]);
    assert_eq!(code, 0);

    // Conflicting with a bad agent: feature branch untouched.
    let r = TestRepo::users();
    r.add_cache_feature();
    r.rename_on_current();
    r.git(&["checkout", "-q", "feature"]);
    let before = r.head();
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        "cat > /dev/null",
        "--max-attempts",
        "1",
        "rebase",
        "main",
    ]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(r.head(), before);
    // With a good agent the rebase succeeds.
    r.gitent(&["abort"]);
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &r.good_cache_agent(),
        "rebase",
        "main",
    ]);
    assert_eq!(code, 0, "{v}");
    assert!(r.read("app/accounts.py").contains("_CACHE"));
}

#[test]
fn revert_after_refactor_retires_requirements() {
    let r = TestRepo::users();
    r.write("app/users.py", USERS_CACHED_PY);
    r.write("tests/test_cache.py", TEST_CACHE_PY);
    r.commit("feat: cache user lookups", &[]);
    let cache = r.head();
    let cache_req = r.note("HEAD")["requirements"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Later refactor keeps the cache but moves everything.
    r.git(&["rm", "-q", "app/users.py"]);
    r.write("app/accounts.py", ACCOUNTS_CACHED_PY);
    r.write("tests/test_users.py", TEST_ACCOUNTS_PY);
    r.write("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY);
    r.commit("refactor: move users to accounts", &[]);

    let (ok, _) = r.git_try(&["revert", "--no-edit", &cache]);
    assert!(!ok, "git revert conflicts after the refactor");
    r.git(&["revert", "--abort"]);

    let agent = r.script(
        "revert_agent.sh",
        &format!(
            "cat > /dev/null\n{}rm tests/test_cache.py",
            heredoc("app/accounts.py", ACCOUNTS_PY)
        ),
    );
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &agent,
        "revert",
        &cache,
    ]);
    assert_eq!(code, 0, "{v}");
    let rec = r.note("HEAD");
    assert_eq!(rec["kind"], "revert");
    assert_eq!(rec["provenance"]["reverts"]["commit"], cache.as_str());
    assert_eq!(rec["retires"][0]["id"], cache_req.as_str());
    assert!(!r.read("app/accounts.py").contains("_CACHE"));
    let (code, chk) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{chk}");
    assert!(chk["results"]
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["id"] != cache_req.as_str()));
}
