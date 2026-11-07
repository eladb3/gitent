//! Criteria I1–I9 (ADR 0013): the fault models from the external evaluation of
//! 2026-11-07, as deterministic tests. One fix on `main` (clamp a discount to
//! [0, 100]) is backported by hand onto `release-1` in many ways, good and bad,
//! and each port is checked with `gitent verify` (no LLM).
//!
//! `main` refactored `calc.py` into a `Cart` class; `release-1` still has
//! `calc.total()` and gained its own `round_money` helper and test.
mod common;
use common::*;

const CALC: &str = "def total(prices, discount):\n    subtotal = sum(prices)\n    return subtotal - subtotal * discount / 100\n";
const CALC_FIXED: &str = "def total(prices, discount):\n    discount = max(0, min(100, discount))\n    subtotal = sum(prices)\n    return subtotal - subtotal * discount / 100\n";
const TEST_CALC: &str = "import unittest\nfrom shop.calc import total\n\n\nclass CalcTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([10, 20], 0), 30)\n";
const CART: &str = "class Cart:\n    def __init__(self, prices):\n        self.prices = list(prices)\n\n    def total(self, discount):\n        subtotal = sum(self.prices)\n        return subtotal - subtotal * discount / 100\n";
const TEST_CART: &str = "import unittest\nfrom shop.cart import Cart\n\n\nclass CartTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(Cart([10, 20]).total(0), 30)\n";
const MONEY: &str = "def round_money(x):\n    return round(x, 2)\n";
const TEST_MONEY: &str = "import unittest\nfrom shop.money import round_money\n\n\nclass MoneyTest(unittest.TestCase):\n    def test_money(self):\n        self.assertEqual(round_money(1.005), 1.0)\n        self.assertEqual(round_money(2.5), 2.5)\n";

/// The ported test, adapted to `calc.total()`.
fn ported_test(body: &str) -> String {
    format!("import unittest\nfrom shop.calc import total\n\n\nclass DiscountTest(unittest.TestCase):\n{body}")
}
const GOOD_TEST_BODY: &str = "    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), 0)\n        self.assertEqual(total([100], -5), 100)\n";

struct Scenario {
    r: TestRepo,
    fix: String,
}

fn scenario() -> Scenario {
    let r = TestRepo::empty();
    r.write("shop/__init__.py", "");
    r.write("tests/__init__.py", "");
    r.write("shop/calc.py", CALC);
    r.write("tests/test_calc.py", TEST_CALC);
    r.write(".gitignore", "__pycache__/\n");
    let (code, v) = r.gitent(&["init"]);
    assert_eq!(code, 0, "{v}");
    r.local_config(
        "[validation]\ntest_command = \"python3 -m unittest discover -q\"\ntest_selector = \"python3 -m unittest -q -k {test} {module}\"\n",
    );
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "v1"]);
    // release-1: its own helper and test.
    r.git(&["checkout", "-qb", "release-1"]);
    r.write("shop/money.py", MONEY);
    r.write("tests/test_money.py", TEST_MONEY);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "release-1: round_money"]);
    // main: refactor into Cart, then the fix.
    r.git(&["checkout", "-q", "main"]);
    r.git(&["rm", "-q", "shop/calc.py", "tests/test_calc.py"]);
    r.write("shop/cart.py", CART);
    r.write("tests/test_cart.py", TEST_CART);
    r.git(&["add", "-A"]);
    r.git(&["commit", "-qm", "refactor: Cart"]);
    r.write(
        "shop/cart.py",
        &CART.replace(
            "        subtotal = sum(self.prices)",
            "        discount = max(0, min(100, discount))\n        subtotal = sum(self.prices)",
        ),
    );
    r.write(
        "tests/test_cart.py",
        &(TEST_CART.to_string()
            + "\n    def test_discount_clamped(self):\n        self.assertEqual(Cart([100]).total(150), 0)\n        self.assertEqual(Cart([100]).total(-5), 100)\n"),
    );
    let v = r.commit("fix: clamp discount to [0, 100]", &[]);
    let req = &v["record"]["requirements"][0];
    assert_eq!(req["test_path"], "tests/test_cart.py", "{v}");
    assert_eq!(req["proof"]["before"], "fail", "{v}");
    let fix = r.head();
    r.git(&["checkout", "-q", "release-1"]);
    r.git(&["checkout", "-qb", "port"]);
    Scenario { r, fix }
}

fn port(s: &Scenario, files: &[(&str, &str)]) -> (i32, serde_json::Value) {
    for (p, c) in files {
        s.r.write(p, c);
    }
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", "port"]);
    s.r.gitent(&["verify", &s.fix])
}

fn check<'a>(v: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    v["validation"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no check {name}: {v}"))
}

#[test]
fn a_proper_port_is_verified() {
    let s = scenario();
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &ported_test(GOOD_TEST_BODY)),
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["verdict"], "verified");
    assert_eq!(v["schema"], "gitent/verify@1");
    assert_eq!(check(&v, "evidence")["passed"], true);
}

#[test]
fn b_weakened_asserts_without_the_fix_are_not_verified() {
    let s = scenario();
    let body =
        "    def test_discount_clamped(self):\n        self.assertIsNotNone(total([100], 150))\n";
    let (code, v) = port(&s, &[("tests/test_discount.py", &ported_test(body))]);
    assert_eq!(code, 3, "{v}");
    assert_eq!(v["verdict"], "unverified");
    let c = check(&v, "tests_intact");
    assert!(
        c["review"] == true && c["detail"].as_str().unwrap().contains("fewer assertions"),
        "{c}"
    );
}

#[test]
fn e_skipped_requirement_test_is_rejected() {
    let s = scenario();
    let body = format!("    @unittest.skip(\"flaky\")\n{GOOD_TEST_BODY}");
    let (code, v) = port(&s, &[("tests/test_discount.py", &ported_test(&body))]);
    assert_eq!(code, 2, "{v}");
    assert!(
        check(&v, "tests_intact")["detail"]
            .as_str()
            .unwrap()
            .contains("skip"),
        "{v}"
    );
}

#[test]
fn f_hard_coded_inputs_pass_the_oracle_documented_limit() {
    // ADR 0013 "out of reach": only a reviewer (human or LLM verifier) sees this.
    let s = scenario();
    let hard = "def total(prices, discount):\n    subtotal = sum(prices)\n    if discount == 150:\n        return 0\n    if discount == -5:\n        return subtotal\n    return subtotal - subtotal * discount / 100\n";
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", hard),
            ("tests/test_discount.py", &ported_test(GOOD_TEST_BODY)),
        ],
    );
    assert_eq!(code, 0, "known limit changed? {v}");
}

#[test]
fn g_renamed_requirement_test_is_rejected() {
    let s = scenario();
    let body = GOOD_TEST_BODY.replace("test_discount_clamped", "test_clamp");
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &ported_test(&body)),
        ],
    );
    assert_eq!(code, 2, "{v}");
    assert_eq!(check(&v, "source_tests_present")["passed"], false);
}

#[test]
fn h_test_added_to_an_existing_file_is_verified() {
    let s = scenario();
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", CALC_FIXED),
            (
                "tests/test_calc.py",
                &(TEST_CALC.to_string() + "\n" + GOOD_TEST_BODY),
            ),
        ],
    );
    assert_eq!(code, 0, "{v}");
}

#[test]
fn i_fix_without_the_test_is_rejected() {
    let s = scenario();
    let (code, v) = port(&s, &[("shop/calc.py", CALC_FIXED)]);
    assert_eq!(code, 2, "{v}");
}

#[test]
fn j_copying_the_source_branch_code_is_not_verified() {
    let s = scenario();
    let fixed_cart = s.r.git(&["show", &format!("{}:shop/cart.py", s.fix)]);
    let cart_test = s.r.git(&["show", &format!("{}:tests/test_cart.py", s.fix)]);
    let (code, v) = port(
        &s,
        &[
            ("shop/cart.py", &fixed_cart),
            ("tests/test_cart.py", &cart_test),
        ],
    );
    assert_eq!(code, 3, "{v}");
    let c = check(&v, "no_orphan_copy");
    assert!(
        c["detail"].as_str().unwrap().contains("shop/cart.py"),
        "{c}"
    );
}

#[test]
fn k_expectations_rewritten_to_match_the_bug_are_not_verified() {
    let s = scenario();
    let body = "    def test_discount_clamped(self):\n        self.assertEqual(total([100], 150), -50)\n        self.assertEqual(total([100], -5), 105)\n";
    let (code, v) = port(&s, &[("tests/test_discount.py", &ported_test(body))]);
    assert_eq!(code, 3, "{v}");
    assert!(
        check(&v, "tests_intact")["detail"]
            .as_str()
            .unwrap()
            .contains("expected values changed"),
        "{v}"
    );
}

#[test]
fn l_weakening_a_target_only_test_is_not_verified() {
    let s = scenario();
    let weaker = TEST_MONEY.replace("        self.assertEqual(round_money(2.5), 2.5)\n", "");
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &ported_test(GOOD_TEST_BODY)),
            ("tests/test_money.py", &weaker),
        ],
    );
    assert_eq!(code, 3, "{v}");
    assert!(
        check(&v, "tests_intact")["detail"]
            .as_str()
            .unwrap()
            .contains("test_money.py::test_money"),
        "{v}"
    );
}

#[test]
fn n_a_same_named_pre_existing_test_does_not_count_as_ported() {
    let s = scenario();
    // A second fix on main whose test happens to share a name with release-1's test_money.
    s.r.git(&["checkout", "-q", "main"]);
    s.r.write(
        "shop/cart.py",
        &(s.r.read("shop/cart.py") + "\n    def count(self):\n        return len(self.prices)\n"),
    );
    s.r.write(
        "tests/test_cart.py",
        &(s.r.read("tests/test_cart.py")
            + "\n    def test_money(self):\n        self.assertEqual(Cart([1, 2]).count(), 2)\n"),
    );
    s.r.commit("feat: Cart.count", &[]);
    let fix2 = s.r.head();
    s.r.git(&["checkout", "-q", "port"]);
    s.r.write("NOTES.md", "nothing ported\n");
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", "port (does nothing)"]);
    let (code, v) = s.r.gitent(&["verify", &fix2]);
    assert_eq!(code, 2, "{v}");
    let c = check(&v, "source_tests_present");
    assert!(c["detail"].as_str().unwrap().contains("not ported"), "{c}");
}

#[test]
fn t_a_fix_without_tests_proves_nothing() {
    let s = scenario();
    s.r.git(&["checkout", "-q", "main"]);
    s.r.write(
        "shop/cart.py",
        &(s.r.read("shop/cart.py") + "\n\nCURRENCY = \"EUR\"\n"),
    );
    s.r.commit("fix: currency is EUR", &[]);
    let fix3 = s.r.head();
    s.r.git(&["checkout", "-q", "port"]);
    s.r.write("shop/money.py", &(MONEY.to_string() + "\n# unrelated\n"));
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", "port (unrelated)"]);
    let (code, v) = s.r.gitent(&["verify", &fix3]);
    assert_eq!(code, 3, "{v}");
    assert_eq!(check(&v, "evidence")["passed"], false);
}

#[test]
fn rev_fake_a_revert_that_only_deletes_the_test_is_rejected() {
    let s = scenario();
    s.r.git(&["checkout", "-q", "main"]);
    s.r.write("tests/test_cart.py", TEST_CART);
    s.r.git(&["add", "-A"]);
    s.r.git(&[
        "commit",
        "-qm",
        &format!("Revert clamp\n\nThis reverts commit {}.", s.fix),
    ]);
    let (code, v) = s.r.gitent(&["verify", &s.fix, "--kind", "revert"]);
    assert_eq!(code, 2, "{v}");
    assert!(
        check(&v, "revert_effective")["detail"]
            .as_str()
            .unwrap()
            .contains("still present"),
        "{v}"
    );

    // A real revert (behaviour and test gone) is verified.
    s.r.write("shop/cart.py", CART);
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-q", "--amend", "--no-edit"]);
    let (code, v) = s.r.gitent(&["verify", &s.fix, "--kind", "revert"]);
    assert_eq!(code, 0, "{v}");
}

#[test]
fn verify_pr_checks_the_whole_pr_and_refuses_self_citations() {
    let s = scenario();
    // Good port with the marker, then a later commit in the same PR skips the test.
    s.r.write("shop/calc.py", CALC_FIXED);
    s.r.write("tests/test_discount.py", &ported_test(GOOD_TEST_BODY));
    s.r.git(&["add", "-A"]);
    s.r.git(&[
        "commit",
        "-qm",
        &format!("clamp discount\n\n(cherry picked from commit {})", s.fix),
    ]);
    let (code, v) = s.r.gitent(&["verify-pr", "release-1"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["schema"], "gitent/verify-pr@1");
    let notes_before = s.r.git(&["notes", "--ref", "gitent", "list"]);
    s.r.write(
        "tests/test_discount.py",
        &ported_test(&format!("    @unittest.skip(\"ci\")\n{GOOD_TEST_BODY}")),
    );
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", "fix CI"]);
    let (code, v) = s.r.gitent(&["verify-pr", "--base", "release-1"]);
    assert_eq!(code, 2, "a later commit breaking the port is seen: {v}");
    assert_eq!(
        s.r.git(&["notes", "--ref", "gitent", "list"]),
        notes_before,
        "verify-pr writes no notes"
    );

    // Citing a commit of the PR itself.
    let own = s.r.head();
    s.r.write("x.txt", "x\n");
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", &format!("more\n\nBackport of {own}")]);
    let (code, v) = s.r.gitent(&["verify-pr", "release-1", "--require-source"]);
    assert_eq!(code, 2, "{v}");
    assert!(
        v["note"].as_str().unwrap().contains("its own commit"),
        "{v}"
    );
}

#[test]
fn llm_verifier_judges_the_result_tree_not_the_users_checkout() {
    let s = scenario();
    s.r.write("shop/calc.py", CALC_FIXED);
    s.r.write("tests/test_discount.py", &ported_test(GOOD_TEST_BODY));
    s.r.git(&["add", "-A"]);
    s.r.git(&["commit", "-qm", "port"]);
    let port = s.r.head();
    s.r.git(&["checkout", "-q", "release-1"]); // the user's checkout lacks the port
    let judge = s.r.script(
        "judge.sh",
        "cat > /dev/null\nif [ -f tests/test_discount.py ]; then echo '{\"satisfied\": true, \"confidence\": 0.9, \"concerns\": []}'; else echo '{\"satisfied\": false, \"confidence\": 0.99, \"concerns\": [\"ran in the wrong tree\"]}'; fi",
    );
    let (code, v) = s.r.gitent_env(
        &["verify", &s.fix, &port],
        &[
            ("GITENT_PROVIDER", "command"),
            ("GITENT_LLM_COMMAND", &judge),
        ],
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(check(&v, "verifier")["passed"], true, "{v}");
}

#[test]
fn stubbing_out_a_target_function_is_not_verified() {
    let s = scenario();
    let (code, v) = port(
        &s,
        &[
            ("shop/calc.py", CALC_FIXED),
            ("tests/test_discount.py", &ported_test(GOOD_TEST_BODY)),
            ("shop/money.py", "def round_money(x):\n    pass\n"),
            ("tests/test_money.py", "import unittest\n"),
        ],
    );
    assert_ne!(code, 0, "{v}");
    let c = check(&v, "bodies_intact");
    assert!(
        c["detail"]
            .as_str()
            .unwrap()
            .contains("shop/money.py::round_money"),
        "{c}"
    );
}
