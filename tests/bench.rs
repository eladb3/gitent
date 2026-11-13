//! Criteria B1–B6 (ADR 0018): `gitent bench` mines tasks from history, runs agents in
//! isolated workspaces and scores them on hidden tests.
mod common;
use common::*;
use serde_json::Value;

const CALC: &str = "def total(prices, discount):\n    subtotal = sum(prices)\n    return subtotal - subtotal * discount / 100\n";
const TEST_CALC: &str = "import unittest\nfrom shop.calc import total\n\n\nclass CalcTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([10, 20], 0), 30)\n";
const CONFIG: &str = "[validation]\ntest_command = \"python3 -m unittest discover -q\"\ntest_selector = \"python3 -m unittest -q -k {test} {module}\"\n";

fn commit(r: &TestRepo, msg: &str) -> String {
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", msg]);
    r.head()
}

/// History: v1, then a fix with a proving test (A), docs, a refactor without tests,
/// a test-only commit, a fix with a vacuous test, and a feature with a test (F).
fn history() -> (TestRepo, String, String) {
    let r = TestRepo::empty();
    r.write("shop/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write("shop/calc.py", CALC);
    r.write("tests/test_calc.py", TEST_CALC);
    r.write(".gitignore", "__pycache__/\n");
    r.write(".gitent/config.toml", CONFIG);
    commit(&r, "v1");
    r.write(
        "shop/calc.py",
        &CALC.replace(
            "    subtotal = sum(prices)",
            "    discount = max(0, min(100, discount))\n    subtotal = sum(prices)",
        ),
    );
    r.write("tests/test_discount.py", "import unittest\nfrom shop.calc import total\n\n\nclass DiscountTest(unittest.TestCase):\n    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), 0)\n        self.assertEqual(total([100], -5), 100)\n");
    let a = commit(&r, "fix: clamp discount to [0, 100]\n\nDiscounts over 100% made totals negative.\n\nSigned-off-by: Dev <dev@example.com>");
    r.write("README.md", "shop\n");
    commit(&r, "docs: readme");
    r.write(
        "shop/calc.py",
        &r.read("shop/calc.py").replace(
            "return subtotal - subtotal * discount / 100",
            "off = subtotal * discount / 100\n    return subtotal - off",
        ),
    );
    commit(&r, "refactor: name the discount amount");
    r.write(
        "tests/test_calc.py",
        &(TEST_CALC.to_string()
            + "\n    def test_empty(self):\n        self.assertEqual(total([], 10), 0)\n"),
    );
    commit(&r, "test: cover empty carts");
    r.write(
        "shop/calc.py",
        &(r.read("shop/calc.py") + "\n\ndef pct(x):\n    return x / 100\n"),
    );
    r.write("tests/test_pct.py", "import unittest\nfrom shop.calc import total\n\n\nclass P(unittest.TestCase):\n    def test_pct_unrelated(self):\n        self.assertEqual(total([1], 0), 1)\n");
    commit(&r, "feat: pct helper (vacuous test)");
    r.write(
        "shop/calc.py",
        &(r.read("shop/calc.py") + "\n\ndef tax(amount):\n    return round(amount * 0.2, 2)\n"),
    );
    r.write("tests/test_tax.py", "import unittest\nfrom shop.calc import tax\n\n\nclass TaxTest(unittest.TestCase):\n    def test_tax(self):\n        self.assertEqual(tax(10), 2.0)\n");
    let f = commit(
        &r,
        "feat: tax()\n\nAdd tax(amount): 20% VAT, rounded to cents.\n\nGitent-Intent: int_123",
    );
    (r, a, f)
}

fn mine(r: &TestRepo) -> (Value, std::path::PathBuf) {
    let out = r.tools.join("bench.json");
    let (code, v) = r.gitent(&["bench", "mine", "--out", out.to_str().unwrap()]);
    assert_eq!(code, 0, "{v}");
    (v, out)
}

#[test]
fn mining_keeps_only_commits_with_proven_tests() {
    let (r, a, f) = history();
    let (v, out) = mine(&r);
    assert_eq!(v["schema"], "gitent/bench@1");
    let tasks = v["tasks"].as_array().unwrap();
    let commits: Vec<&str> = tasks
        .iter()
        .map(|t| t["commit"].as_str().unwrap())
        .collect();
    assert_eq!(commits, vec![f.as_str(), a.as_str()], "{v}");
    let fix = &tasks[1];
    assert_eq!(fix["hidden_tests"][0]["test"], "test_discount_clamped");
    assert_eq!(fix["hidden_tests"][0]["proof"], "assertion");
    assert_eq!(
        fix["statement"],
        "fix: clamp discount to [0, 100]\n\nDiscounts over 100% made totals negative."
    );
    assert_eq!(tasks[0]["hidden_tests"][0]["proof"], "missing_api");
    assert!(!tasks[0]["statement"]
        .as_str()
        .unwrap()
        .contains("Gitent-Intent"));
    let skipped = v["skipped"].to_string();
    for why in [
        "no code change",
        "no test added or modified",
        "no test fails without the commit",
    ] {
        assert!(skipped.contains(why), "{why}: {skipped}");
    }
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
    assert_eq!(saved["tasks"].as_array().unwrap().len(), 2);
    // --limit
    let (code, v) = r.gitent(&["bench", "mine", "--limit", "1"]);
    assert_eq!(code, 0);
    assert_eq!(v["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn agents_are_scored_on_hidden_tests_in_isolated_workspaces() {
    let (r, a, _) = history();
    let (_, bench) = mine(&r);
    let log = r.tools.join("agent.log");
    // A capable stand-in agent: makes each fix from the statement alone. It also records
    // what it can see: the workspace must hold no history and no hidden tests.
    let good = r.script(
        "good.sh",
        &format!(
            r#"brief=$(cat)
{{ echo "commits=$(git rev-list --all | wc -l | tr -d ' ')"; git cat-file -e {a} 2>/dev/null && echo leak=yes || echo leak=no; case "$brief" in *clamp*) h=tests/test_discount.py ;; *) h=tests/test_tax.py ;; esac; [ -e "$h" ] && echo hidden=visible || echo hidden=absent; }} >> {log}
case "$brief" in
  *clamp*) python3 - <<'PY'
p = "shop/calc.py"; s = open(p).read()
s = s.replace("    subtotal = sum(prices)", "    discount = max(0, min(100, discount))\n    subtotal = sum(prices)", 1)
open(p, "w").write(s)
PY
  ;;
  *tax*) printf '\n\ndef tax(amount):\n    return round(amount * 0.2, 2)\n' >> shop/calc.py ;;
esac
echo done"#,
            log = log.display()
        ),
    );
    let lazy = r.script("lazy.sh", "cat > /dev/null\necho 'All done, tests pass.'");
    let good_out = r.tools.join("good.json");
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &good,
        "bench",
        "run",
        bench.to_str().unwrap(),
        "--out",
        good_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["schema"], "gitent/bench-run@1");
    assert_eq!(v["resolved"], 2, "{v}");
    assert_eq!(v["false_claims"], 0);
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(
        seen.contains("commits=1") && !seen.contains("commits=2"),
        "one commit, no history: {seen}"
    );
    assert!(
        seen.contains("leak=no") && !seen.contains("leak=yes"),
        "the fix commit is unreachable: {seen}"
    );
    assert!(
        seen.contains("hidden=absent") && !seen.contains("hidden=visible"),
        "{seen}"
    );

    let lazy_out = r.tools.join("lazy.json");
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &lazy,
        "bench",
        "run",
        bench.to_str().unwrap(),
        "--out",
        lazy_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["resolved"], 0, "{v}");
    assert_eq!(v["false_claims"], 2, "said done, hidden tests fail: {v}");

    // compare
    let (code, v) = r.gitent(&[
        "bench",
        "compare",
        good_out.to_str().unwrap(),
        lazy_out.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "{v}");
    let md = v["markdown"].as_str().unwrap();
    assert!(md.contains("| **resolved** | 2/2 | 0/2 |"), "{md}");
    assert!(md.contains("❌ claimed done"), "{md}");

    // --tasks selects a subset; the user's checkout is untouched throughout
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &good,
        "bench",
        "run",
        bench.to_str().unwrap(),
        "--tasks",
        &a[..7],
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["total"], 1);
    assert_eq!(r.git(&["status", "--porcelain"]), "");
}

#[test]
fn an_agent_writing_its_own_weak_tests_is_still_scored_on_the_hidden_ones() {
    // The agent "fixes" nothing but adds a passing test under the hidden test's name;
    // the commit's test files replace it before scoring.
    let (r, a, _) = history();
    let (_, bench) = mine(&r);
    let cheat = r.script(
        "cheat.sh",
        "cat > /dev/null\nprintf 'import unittest\\n\\n\\nclass DiscountTest(unittest.TestCase):\\n    def test_discount_clamped(self):\\n        self.assertTrue(True)\\n' > tests/test_discount.py\necho done",
    );
    let (code, v) = r.gitent(&[
        "--executor",
        "command",
        "--executor-command",
        &cheat,
        "bench",
        "run",
        bench.to_str().unwrap(),
        "--tasks",
        &a[..7],
    ]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["resolved"], 0, "{v}");
    assert_eq!(v["false_claims"], 1);
}

#[test]
fn bench_needs_an_agent_and_is_cli_only() {
    let (r, _, _) = history();
    let (_, bench) = mine(&r);
    let (code, v) = r.gitent(&[
        "--executor",
        "host",
        "bench",
        "run",
        bench.to_str().unwrap(),
    ]);
    assert_eq!(code, 1, "{v}");
    assert!(
        v["error"].as_str().unwrap().contains("needs an agent"),
        "{v}"
    );
}
