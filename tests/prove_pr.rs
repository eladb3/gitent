//! Criteria P1–P9 (ADR 0017): `gitent prove-pr` on ordinary pull requests.
mod common;
use common::*;
use serde_json::Value;

const CALC: &str = "def total(prices, discount):\n    subtotal = sum(prices)\n    return subtotal - subtotal * discount / 100\n";
const CALC_FIXED: &str = "def total(prices, discount):\n    discount = max(0, min(100, discount))\n    subtotal = sum(prices)\n    return subtotal - subtotal * discount / 100\n";
const TEST_CALC: &str = "import unittest\nfrom shop.calc import total\n\n\nclass CalcTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([10, 20], 0), 30)\n";
const CONFIG: &str = "[validation]\ntest_command = \"python3 -m unittest discover -q\"\ntest_selector = \"python3 -m unittest -q -k {test} {module}\"\n";

fn test_file(body: &str) -> String {
    format!("import unittest\nfrom shop.calc import total\n\n\nclass DiscountTest(unittest.TestCase):\n{body}")
}
const CLAMP_TEST: &str = "    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), 0)\n        self.assertEqual(total([100], -5), 100)\n";

/// main with calc.py + one test, a shared config; branch `pr` checked out.
fn repo() -> TestRepo {
    let r = TestRepo::empty();
    r.write("shop/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write("shop/calc.py", CALC);
    r.write("tests/test_calc.py", TEST_CALC);
    r.write(".gitignore", "__pycache__/\n");
    r.write(".gitent/config.toml", CONFIG);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "v1"]);
    r.git(&["checkout", "-qb", "pr"]);
    r
}

fn pr(r: &TestRepo, files: &[(&str, &str)]) -> (i32, Value) {
    for (p, c) in files {
        r.write(p, c);
    }
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "the PR"]);
    r.gitent(&["prove-pr", "main"])
}

fn status<'a>(v: &'a Value, test: &str) -> &'a str {
    v["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["test"] == test)
        .unwrap_or_else(|| panic!("no test {test}: {v}"))["status"]
        .as_str()
        .unwrap()
}

#[test]
fn a_fix_with_a_test_that_fails_without_it_is_proven() {
    let r = repo();
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(CLAMP_TEST)),
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"], "proven");
    assert_eq!(v["schema"], "gitent/prove-pr@1");
    assert_eq!(status(&v, "test_discount_clamped"), "proven");
    assert!(v["markdown"]
        .as_str()
        .unwrap()
        .starts_with("<!-- gitent-prove-pr -->"));
    // read-only: no notes
    let (ok, _) = r.git_try(&["notes", "--ref", "gitent", "list"]);
    assert!(!ok || r.git(&["notes", "--ref", "gitent", "list"]).is_empty());
}

#[test]
fn a_test_that_passes_without_the_change_proves_nothing() {
    let r = repo();
    let vacuous =
        "    def test_discount_clamped(self):\n        self.assertEqual(total([10, 20], 0), 30)\n";
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(vacuous)),
        ],
    );
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["result"], "unproven");
    assert_eq!(status(&v, "test_discount_clamped"), "passes_without_change");
}

#[test]
fn code_without_tests_is_unproven_and_docs_only_is_neutral() {
    let r = repo();
    let (code, v) = pr(&r, &[("shop/calc.py", CALC_FIXED)]);
    assert_eq!(code, 3, "{v}");
    assert!(
        v["reason"].as_str().unwrap().contains("no test was added"),
        "{v}"
    );
    let r = repo();
    let (code, v) = pr(&r, &[("README.md", "docs\n")]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["result"], "neutral");
}

#[test]
fn a_test_failing_on_the_pr_or_a_skipped_existing_test_is_rejected() {
    let r = repo();
    let wrong =
        "    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), 1)\n";
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(wrong)),
        ],
    );
    assert_eq!(code, 2, "{v}");
    assert_eq!(status(&v, "test_discount_clamped"), "fails_on_pr");

    let r = repo();
    let skipped = TEST_CALC.replace(
        "    def test_total",
        "    @unittest.skip(\"later\")\n    def test_total",
    );
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_calc.py", &skipped),
            ("tests/test_discount.py", &test_file(CLAMP_TEST)),
        ],
    );
    assert_eq!(code, 2, "{v}");
    assert!(
        v["skipped_existing"][0]
            .as_str()
            .unwrap()
            .contains("test_total"),
        "{v}"
    );
}

#[test]
fn deleting_an_existing_test_is_unproven_but_moving_it_is_fine() {
    let r = repo();
    r.rm("tests/test_calc.py");
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(CLAMP_TEST)),
        ],
    );
    assert_eq!(code, 3, "{v}");
    assert!(
        v["deleted_tests"][0]
            .as_str()
            .unwrap()
            .contains("test_total"),
        "{v}"
    );

    let r = repo();
    r.rm("tests/test_calc.py");
    let both = test_file(&format!(
        "{CLAMP_TEST}\n    def test_total(self):\n        self.assertEqual(total([10, 20], 0), 30)\n"
    ));
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &both),
        ],
    );
    assert_eq!(code, 0, "moved, not deleted: {v}");
}

#[test]
fn characterization_tests_are_listed_and_require_all_makes_them_count() {
    let r = repo();
    let mixed = format!(
        "{CLAMP_TEST}\n    def test_zero_discount(self):\n        self.assertEqual(total([10], 0), 10)\n"
    );
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(&mixed)),
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(status(&v, "test_zero_discount"), "passes_without_change");
    let (code, v) = r.gitent(&["prove-pr", "main", "--require-all"]);
    assert_eq!(code, 3, "{v}");
}

#[test]
fn a_new_feature_is_proven_by_its_missing_api() {
    let r = repo();
    let feature = format!("{CALC}\n\ndef tax(amount):\n    return amount * 0.2\n");
    let t = "import unittest\nfrom shop.calc import tax\n\n\nclass TaxTest(unittest.TestCase):\n    def test_tax(self):\n        self.assertEqual(tax(100), 20)\n";
    let (code, v) = pr(&r, &[("shop/calc.py", &feature), ("tests/test_tax.py", t)]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(status(&v, "test_tax"), "proven_api");
}

#[test]
fn the_pr_cannot_choose_how_it_is_proved() {
    // ADR 0017 §6: config comes from the base; a PR turning the selector into `true`
    // would make every test "pass" everywhere.
    let r = repo();
    let wrong =
        "    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), 1)\n";
    let (code, v) = pr(
        &r,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &test_file(wrong)),
            (
                ".gitent/config.toml",
                "[validation]\ntest_command = \"true\"\ntest_selector = \"true\"\n",
            ),
        ],
    );
    assert_eq!(code, 2, "base config still runs the real test: {v}");
}

#[test]
fn working_tree_mode_proves_uncommitted_edits_without_touching_them() {
    let r = repo();
    r.write("shop/calc.py", CALC_FIXED);
    r.write("tests/test_discount.py", &test_file(CLAMP_TEST));
    let before = r.git(&["status", "--porcelain"]);
    let (code, v) = r.gitent(&["prove-pr", "main", "--working-tree"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["head"], "working-tree");
    assert_eq!(
        r.git(&["status", "--porcelain"]),
        before,
        "files and index untouched"
    );
}

#[test]
fn available_over_mcp() {
    use std::io::Write;
    let r = repo();
    for (p, c) in [
        ("shop/calc.py", CALC_FIXED.to_string()),
        ("tests/test_discount.py", test_file(CLAMP_TEST)),
    ] {
        r.write(p, &c);
    }
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "pr"]);
    let mut child = std::process::Command::new(bin())
        .args(["-C", r.path.to_str().unwrap(), "mcp"])
        .env("TMPDIR", r.path.join(".git"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"gitent_prove_pr","arguments":{{"base":"main"}}}}}}"#).unwrap();
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("gitent_prove_pr"), "{text}");
    assert!(text.contains("tests prove the change"), "{text}");
}
