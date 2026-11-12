#!/usr/bin/env bash
# Build a PR-shaped repository for the prove-action self-test.
#   prove_fixture.sh <dir> proven|vacuous|no-test|skip-existing|docs
# `main` has shop/calc.py + tests; branch `pr` is checked out at the end.
set -euo pipefail
dir="$1"; scenario="$2"
rm -rf "$dir"; mkdir -p "$dir"; cd "$dir"
git init -q -b main
git config user.email selftest@example.com; git config user.name selftest
mkdir -p shop tests; : > shop/__init__.py; : > tests/__init__.py
printf '__pycache__/\n' > .gitignore
cat > shop/calc.py <<'PY'
def total(prices, discount):
    subtotal = sum(prices)
    return subtotal - subtotal * discount / 100
PY
cat > tests/test_calc.py <<'PY'
import unittest
from shop.calc import total


class CalcTest(unittest.TestCase):
    def test_total(self):
        self.assertEqual(total([10, 20], 0), 30)
PY
git add -A; git commit -qm v1
git checkout -qb pr
fix() {
  cat > shop/calc.py <<'PY'
def total(prices, discount):
    discount = max(0, min(100, discount))
    subtotal = sum(prices)
    return subtotal - subtotal * discount / 100
PY
}
discount_test() { # $1 = assertion lines
  printf 'import unittest\nfrom shop.calc import total\n\n\nclass DiscountTest(unittest.TestCase):\n    def test_discount_clamped(self):\n%s\n' "$1" > tests/test_discount.py
}
case "$scenario" in
  proven)  fix; discount_test '        self.assertEqual(total([100], 150), 0)' ;;
  vacuous) fix; discount_test '        self.assertEqual(total([10, 20], 0), 30)' ;;
  no-test) fix ;;
  skip-existing)
    fix; discount_test '        self.assertEqual(total([100], 150), 0)'
    sed -i 's/    def test_total/    @unittest.skip("later")\n    def test_total/' tests/test_calc.py ;;
  docs) echo "docs" > README.md ;;
  *) echo "unknown scenario $scenario" >&2; exit 2 ;;
esac
git add -A; git commit -qm "the PR ($scenario)"
