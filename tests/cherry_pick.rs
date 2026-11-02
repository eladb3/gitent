//! Criteria C (semantic cherry-pick) and R6, using real git + real processes as agents.
mod common;
use common::*;
use serde_json::Value;

fn pick(r: &TestRepo, rev: &str, agent: &str, extra: &[&str]) -> (i32, Value) {
    let mut a = vec![
        "--executor",
        "command",
        "--executor-command",
        agent,
        "cherry-pick",
        rev,
    ];
    a.extend_from_slice(extra);
    r.gitent(&a)
}

#[test]
fn clean_pick_uses_git_and_no_agent() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.write("README.md", "users service\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "docs"]);
    let marker = r.tools.join("agent_was_called");
    let agent = r.script(
        "marker_agent.sh",
        &format!("touch {}; exit 1", marker.display()),
    );
    let (code, v) = pick(&r, &feature, &agent, &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "succeeded");
    assert!(
        !marker.exists(),
        "agent must not be invoked when git + validation succeed"
    );
    let rec = r.note("HEAD");
    assert_eq!(rec["provenance"]["executor"], "git");
    assert_eq!(
        rec["provenance"]["derived_from"]["commit"],
        feature.as_str()
    );
    assert_eq!(r.git(&["status", "--porcelain"]), "");
    assert_eq!(r.worktrees(), 1, "worktree cleaned up");
}

#[test]
fn conflict_reimplemented_by_agent_with_provenance() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let target = r.head();

    // Plain git conflicts (modify/delete after the rename).
    let (ok, out) = r.git_try(&["cherry-pick", &feature]);
    assert!(!ok, "git cherry-pick should conflict: {out}");
    r.git(&["cherry-pick", "--abort"]);

    let (code, v) = pick(&r, &feature, &r.good_cache_agent(), &[]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "succeeded");
    let step = &v["steps"][0];
    assert_eq!(step["attempts"][0]["by"], "git");
    assert_eq!(
        step["attempts"][0]["validation"]["checks"][0]["name"],
        "git_apply"
    );
    assert_eq!(step["attempts"][1]["validation"]["passed"], true);

    // Branch moved to a new commit on top of the target; working tree updated.
    assert_eq!(r.rev("HEAD~1"), target);
    assert!(r.read("app/accounts.py").contains("_CACHE"));
    assert_eq!(r.git(&["status", "--porcelain"]), "");

    // C7 provenance + R6 requirement identity.
    let src = r.note(&feature);
    let rec = r.note("HEAD");
    assert_eq!(rec["intent_id"], src["intent_id"]);
    assert_eq!(rec["provenance"]["operation"], "cherry-pick");
    assert_eq!(
        rec["provenance"]["derived_from"]["commit"],
        feature.as_str()
    );
    assert!(rec["provenance"]["executor"]
        .as_str()
        .unwrap()
        .starts_with("command:"));
    assert_eq!(rec["provenance"]["validation"]["passed"], true);
    let q = &rec["requirements"][0];
    assert_eq!(q["id"], src["requirements"][0]["id"]);
    assert_eq!(q["rebound_from"], feature.as_str());
    assert_eq!(
        q["proof"]["before"], "fail",
        "ported test discriminates on the target"
    );
    // The ledger counts the requirement once and it holds.
    let (code, chk) = r.gitent(&["check"]);
    assert_eq!(code, 0, "{chk}");
    assert_eq!(chk["results"].as_array().unwrap().len(), 1);
    let msg = r.git(&["log", "-1", "--format=%B"]);
    assert!(msg.contains(&format!("Gitent-Derived-From: {feature}")));
}

const PRICING_PY: &str = r#"def shipping(items):
    """Flat shipping, in dollars."""
    return 5


def subtotal(items):
    """Sum of price * quantity, in dollars."""
    return sum(price * qty for price, qty in items)


def item_count(items):
    return sum(qty for _, qty in items)


def total(items):
    return subtotal(items) + shipping(items)
"#;

const TEST_PRICING_PY: &str = r#"import unittest
from app.pricing import total


class PricingTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total([(1.5, 2)]), 8)
"#;

/// Feature: 10% off subtotals over 100 (in dollars).
const PRICING_DISCOUNT_PY: &str = r#"def shipping(items):
    """Flat shipping, in dollars."""
    return 5


def subtotal(items):
    """Sum of price * quantity, in dollars."""
    return sum(price * qty for price, qty in items)


def item_count(items):
    return sum(qty for _, qty in items)


def total(items):
    sub = subtotal(items)
    if sub > 100:
        sub = sub * 0.9
    return sub + shipping(items)
"#;

const TEST_DISCOUNT_PY: &str = r#"import unittest
from app.pricing import total


class DiscountTest(unittest.TestCase):
    def test_discount_over_100(self):
        self.assertEqual(total([(60, 2)]), 113)
"#;

/// Target: everything in integer cents.
const PRICING_CENTS_PY: &str = r#"def shipping(items):
    """Flat shipping, in cents."""
    return 500


def subtotal(items):
    """Sum of price * quantity, in cents."""
    return sum(round(price * 100) * qty for price, qty in items)


def item_count(items):
    return sum(qty for _, qty in items)


def total(items):
    return subtotal(items) + shipping(items)
"#;

const TEST_PRICING_CENTS_PY: &str = r#"import unittest
from app.pricing import total


class PricingTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total([(1.5, 2)]), 800)
"#;

const PRICING_CENTS_DISCOUNT_PY: &str = r#"def shipping(items):
    """Flat shipping, in cents."""
    return 500


def subtotal(items):
    """Sum of price * quantity, in cents."""
    return sum(round(price * 100) * qty for price, qty in items)


def item_count(items):
    return sum(qty for _, qty in items)


def total(items):
    sub = subtotal(items)
    if sub > 100_00:
        sub = round(sub * 0.9)
    return sub + shipping(items)
"#;

const TEST_DISCOUNT_CENTS_PY: &str = r#"import unittest
from app.pricing import total


class DiscountTest(unittest.TestCase):
    def test_discount_over_100(self):
        self.assertEqual(total([(60, 2)]), 113_00)
"#;

#[test]
fn clean_but_wrong_pick_escalates_to_agent() {
    let r = TestRepo::empty();
    r.write("app/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write("app/pricing.py", PRICING_PY);
    r.write("tests/test_pricing.py", TEST_PRICING_PY);
    r.write(".gitignore", "__pycache__/\n");
    r.gitent(&["init"]);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "base: pricing"]);
    r.git(&["checkout", "-qb", "discount"]);
    r.write("app/pricing.py", PRICING_DISCOUNT_PY);
    r.write("tests/test_discount.py", TEST_DISCOUNT_PY);
    r.commit("feat: 10% discount on orders over $100", &[]);
    let feature = r.head();
    r.git(&["checkout", "-q", "main"]);
    r.write("app/pricing.py", PRICING_CENTS_PY);
    r.write("tests/test_pricing.py", TEST_PRICING_CENTS_PY);
    r.git(&["commit", "-qam", "refactor: money in integer cents"]);

    // Plain git applies cleanly — and is silently wrong.
    let (ok, _) = r.git_try(&["cherry-pick", "--no-commit", &feature]);
    assert!(ok, "git applies the patch without conflicts");
    let (ok, _) = r.git_try(&["-c", "core.hooksPath=/dev/null", "status"]);
    assert!(ok);
    let o = std::process::Command::new("python3")
        .args(["-m", "unittest", "discover", "-q"])
        .current_dir(&r.path)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(!o.status.success(), "git's textual result fails the tests");
    r.git(&["reset", "-q", "--hard"]);
    r.git(&["clean", "-qfd"]);

    let agent = r.script(
        "cents_agent.sh",
        &format!(
            "cat > /dev/null\n{}{}",
            heredoc("app/pricing.py", PRICING_CENTS_DISCOUNT_PY),
            heredoc("tests/test_discount.py", TEST_DISCOUNT_CENTS_PY)
        ),
    );
    let (code, v) = pick(&r, &feature, &agent, &[]);
    assert_eq!(code, 0, "{v}");
    let git_attempt = &v["steps"][0]["attempts"][0];
    assert_eq!(git_attempt["by"], "git");
    assert_eq!(
        git_attempt["validation"]["passed"], false,
        "clean textual pick was rejected by validation"
    );
    let failed: Vec<&str> = git_attempt["validation"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["passed"] == false)
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert!(
        failed.contains(&"tests") && failed.contains(&"requirements"),
        "{failed:?}"
    );
    assert!(r.read("app/pricing.py").contains("100_00"));
}

#[test]
fn bad_agent_is_retried_then_rejected() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let target = r.head();
    let counter = r.tools.join("calls");
    let agent = r.script(
        "bad_agent.sh",
        &format!(
            "cat > /dev/null\necho x >> {}\n{}",
            counter.display(),
            heredoc("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY) // test ported, but no cache implemented
        ),
    );
    let (code, v) = pick(&r, &feature, &agent, &["--max-attempts", "2"]);
    assert_eq!(code, 2, "{v}");
    assert_eq!(v["state"], "failed");
    assert!(v["error"].as_str().unwrap().contains("rejected"));
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        2,
        "retried with feedback"
    );
    let brief = std::fs::read_to_string(v["steps"][0]["brief_path"].as_str().unwrap()).unwrap();
    assert!(
        brief.contains("Feedback from previous attempts"),
        "second brief carries feedback"
    );
    assert_eq!(r.head(), target, "branch untouched");
    assert_eq!(r.git(&["status", "--porcelain"]), "");
    let wt = v["worktree"].as_str().unwrap().to_string();
    assert!(
        std::path::Path::new(&wt).exists(),
        "worktree kept for inspection"
    );
    let (code, _) = r.gitent(&["abort"]);
    assert_eq!(code, 0);
    assert!(!std::path::Path::new(&wt).exists());
}

#[test]
fn deleting_tests_is_rejected() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    // Implements caching but "makes it pass" by deleting a test and skipping the ported one.
    let agent = r.script(
        "cheat_agent.sh",
        &(format!(
            "cat > /dev/null\n{}{}",
            heredoc("app/accounts.py", ACCOUNTS_CACHED_PY),
            heredoc(
                "tests/test_users.py",
                "import unittest\nfrom app.accounts import fetch_user\n\n\nclass UsersTest(unittest.TestCase):\n    def test_get_user(self):\n        self.assertEqual(fetch_user(1)[\"name\"], \"user1\")\n"
            )
        ) + "rm -f tests/test_cache.py\n"),
    );
    let (code, v) = pick(&r, &feature, &agent, &["--max-attempts", "1"]);
    assert_eq!(code, 2, "{v}");
    let checks = &v["steps"][0]["attempts"][1]["validation"]["checks"];
    let failed = |n: &str| {
        checks
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == n && c["passed"] == false)
    };
    assert!(failed("existing_tests_preserved"), "{checks}");
    assert!(failed("source_tests_present"), "{checks}");
    assert!(failed("requirements"), "{checks}");
}

#[test]
fn dirty_tree_is_refused() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.write("app/db.py", "# local edit\n");
    let (code, v) = pick(&r, &feature, "true", &[]);
    assert_eq!(code, 1);
    assert!(
        v["error"].as_str().unwrap().contains("uncommitted changes"),
        "{v}"
    );
    assert_eq!(
        r.read("app/db.py"),
        "# local edit\n",
        "local edit untouched"
    );
}

#[test]
fn host_mode_roundtrip() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let (code, v) = r.gitent(&["cherry-pick", &feature]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "awaiting_host");
    let wt = std::path::PathBuf::from(v["worktree"].as_str().unwrap());
    let brief = std::fs::read_to_string(v["brief"].as_str().unwrap()).unwrap();
    let req_id = r.note(&feature)["requirements"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        brief.contains("Definition of done") && brief.contains(&req_id),
        "{brief}"
    );
    assert!(
        brief.contains("fetch_user") || brief.contains("renamed"),
        "brief points at the new code"
    );
    let (code, v2) = r.gitent(&["status"]);
    assert_eq!(code, 0);
    assert_eq!(v2["open_operations"].as_array().unwrap().len(), 1);

    // First host attempt is wrong: feedback, still awaiting.
    std::fs::write(wt.join("tests/test_cache.py"), TEST_CACHE_ACCOUNTS_PY).unwrap();
    let (code, v) = r.gitent(&["continue"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "awaiting_host");
    let brief = std::fs::read_to_string(v["brief"].as_str().unwrap()).unwrap();
    assert!(brief.contains("Feedback from previous attempts"));

    // Second attempt is right.
    std::fs::write(wt.join("app/accounts.py"), ACCOUNTS_CACHED_PY).unwrap();
    let (code, v) = r.gitent(&["continue"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "succeeded");
    assert_eq!(r.note("HEAD")["provenance"]["executor"], "host");
    assert!(r.read("app/accounts.py").contains("_CACHE"));
}

#[test]
fn branch_moved_during_operation_aborts() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let (_, v) = r.gitent(&["cherry-pick", &feature]);
    let wt = std::path::PathBuf::from(v["worktree"].as_str().unwrap());
    // Meanwhile someone commits to main.
    r.write("NOTES.md", "x\n");
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "concurrent work"]);
    let moved = r.head();
    std::fs::write(wt.join("app/accounts.py"), ACCOUNTS_CACHED_PY).unwrap();
    std::fs::write(wt.join("tests/test_cache.py"), TEST_CACHE_ACCOUNTS_PY).unwrap();
    let (code, v) = r.gitent(&["continue"]);
    assert_eq!(code, 2, "{v}");
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("moved during the operation"));
    assert_eq!(r.head(), moved, "concurrent commit preserved");
}

#[test]
fn builtin_agent_with_scripted_provider() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let act = |tool: &str, args: Value| serde_json::json!({"when": "coding agent", "response": {"tool": tool, "args": args}});
    let script = serde_json::json!([
        act("search", serde_json::json!({"pattern": "def fetch_user"})),
        act("read_file", serde_json::json!({"path": "app/accounts.py"})),
        act("write_file", serde_json::json!({"path": "app/accounts.py", "content": ACCOUNTS_CACHED_PY})),
        act("write_file", serde_json::json!({"path": "tests/test_cache.py", "content": TEST_CACHE_ACCOUNTS_PY})),
        act("run", serde_json::json!({"command": "python3 -m unittest discover -q"})),
        act("finish", serde_json::json!({"summary": "cached fetch_user"})),
        {"when": "Gitent's verifier", "response": {"satisfied": true, "confidence": 0.9, "concerns": []}}
    ]);
    let f = r.tools.join("script.json");
    std::fs::write(&f, script.to_string()).unwrap();
    r.local_config(&format!(
        "[llm]\nprovider = \"scripted\"\nscript = \"{}\"\n[executor]\nkind = \"builtin\"\n",
        f.display()
    ));
    let (code, v) = r.gitent(&["cherry-pick", &feature]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["state"], "succeeded");
    let rec = r.note("HEAD");
    assert_eq!(rec["provenance"]["executor"], "builtin:scripted");
    let checks = rec["provenance"]["validation"]["checks"]
        .as_array()
        .unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["name"] == "verifier" && c["passed"] == true),
        "{checks:?}"
    );
}
