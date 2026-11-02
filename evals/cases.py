"""Eval scenarios: diverged histories where a change must be moved.

Each case builds a repository history, names one operation, and ships HIDDEN
tests (never visible to any arm) that define whether the intended behaviour was
reconstructed on the target. `reference` is a known-good solution used by the
offline agent to validate the harness itself.
"""

USERS_BASE = {
    "app/__init__.py": "",
    "tests/__init__.py": "",
    ".gitignore": "__pycache__/\n",
    "app/db.py": 'CALLS = {"n": 0}\n\n\ndef load_user(uid):\n    CALLS["n"] += 1\n    return {"id": uid, "name": f"user{uid}"}\n',
    "app/users.py": 'from app import db\n\n\ndef get_user(uid):\n    return db.load_user(uid)\n\n\ndef display_name(uid):\n    return get_user(uid)["name"].title()\n',
    "tests/test_users.py": 'import unittest\nfrom app.users import get_user, display_name\n\n\nclass UsersTest(unittest.TestCase):\n    def test_get_user(self):\n        self.assertEqual(get_user(1)["name"], "user1")\n\n    def test_display_name(self):\n        self.assertEqual(display_name(2), "User2")\n',
}

USERS_CACHED = 'from app import db\n\n_CACHE = {}\n\n\ndef get_user(uid):\n    """Return the user, loading it from the database at most once."""\n    if uid not in _CACHE:\n        _CACHE[uid] = db.load_user(uid)\n    return _CACHE[uid]\n\n\ndef display_name(uid):\n    return get_user(uid)["name"].title()\n'
TEST_CACHE = 'import unittest\nfrom app import db\nfrom app.users import get_user\n\n\nclass CacheTest(unittest.TestCase):\n    def test_cache_hit(self):\n        before = db.CALLS["n"]\n        get_user(42)\n        get_user(42)\n        self.assertEqual(db.CALLS["n"] - before, 1)\n'

ACCOUNTS = '"""Account lookups (formerly app/users.py)."""\nfrom app import db\n\n\ndef fetch_user(user_id: int) -> dict:\n    """Return the user record for ``user_id``."""\n    record = db.load_user(user_id)\n    record.setdefault("active", True)\n    return record\n\n\ndef display_name(user_id: int) -> str:\n    return fetch_user(user_id)["name"].title()\n'
TEST_ACCOUNTS = 'import unittest\nfrom app.accounts import fetch_user, display_name\n\n\nclass UsersTest(unittest.TestCase):\n    def test_get_user(self):\n        self.assertEqual(fetch_user(1)["name"], "user1")\n\n    def test_display_name(self):\n        self.assertEqual(display_name(2), "User2")\n\n    def test_active_default(self):\n        self.assertTrue(fetch_user(3)["active"])\n'
ACCOUNTS_CACHED = '"""Account lookups (formerly app/users.py)."""\nfrom app import db\n\n_CACHE = {}\n\n\ndef fetch_user(user_id: int) -> dict:\n    """Return the user record for ``user_id`` (cached)."""\n    if user_id not in _CACHE:\n        record = db.load_user(user_id)\n        record.setdefault("active", True)\n        _CACHE[user_id] = record\n    return _CACHE[user_id]\n\n\ndef display_name(user_id: int) -> str:\n    return fetch_user(user_id)["name"].title()\n'
TEST_CACHE_ACCOUNTS = TEST_CACHE.replace("from app.users import get_user", "from app.accounts import fetch_user").replace("get_user(42)", "fetch_user(42)")

CASES = []

# ---------------------------------------------------------------- E0 parity
CASES.append({
    "id": "E0-parity-clean-pick",
    "description": "Control: the target only changed unrelated files. Git's cherry-pick is correct; Gitent must match it without any agent call.",
    "kind": "parity",
    "base": USERS_BASE,
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: cache user lookups\n\nLoad each user from the database at most once per process.",
         "files": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE}},
        {"branch": "main", "message": "docs: add README", "files": {"README.md": "# users service\n"}},
    ],
    "operation": {"op": "cherry-pick", "rev": "feature"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import db\nfrom app.users import get_user, display_name\n\n\nclass Hidden(unittest.TestCase):\n    def test_second_lookup_cached(self):\n        b = db.CALLS["n"]\n        get_user(7)\n        get_user(7)\n        self.assertEqual(db.CALLS["n"] - b, 1)\n\n    def test_distinct_ids(self):\n        b = db.CALLS["n"]\n        get_user(8)\n        get_user(9)\n        self.assertEqual(db.CALLS["n"] - b, 2)\n\n    def test_display(self):\n        self.assertEqual(display_name(11), "User11")\n'},
    "reference": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE},
})

# ---------------------------------------------------------------- E1 rename / move
CASES.append({
    "id": "E1-pick-after-rename-move",
    "description": "Cherry-pick caching onto a branch where get_user was renamed to fetch_user and moved to app/accounts.py (git: modify/delete conflict).",
    "kind": "recovery",
    "base": USERS_BASE,
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: cache user lookups\n\nLoad each user from the database at most once per process.",
         "files": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE}},
        {"branch": "main", "message": "refactor: rename get_user to fetch_user and move to app/accounts.py",
         "files": {"app/users.py": None, "app/accounts.py": ACCOUNTS, "tests/test_users.py": TEST_ACCOUNTS}},
    ],
    "operation": {"op": "cherry-pick", "rev": "feature"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import db\nfrom app.accounts import fetch_user, display_name\n\n\nclass Hidden(unittest.TestCase):\n    def test_second_lookup_cached(self):\n        b = db.CALLS["n"]\n        a = fetch_user(7)\n        c = fetch_user(7)\n        self.assertEqual(db.CALLS["n"] - b, 1)\n        self.assertEqual(a, c)\n\n    def test_distinct_ids(self):\n        b = db.CALLS["n"]\n        fetch_user(8)\n        fetch_user(9)\n        self.assertEqual(db.CALLS["n"] - b, 2)\n\n    def test_active_default_kept(self):\n        self.assertTrue(fetch_user(10)["active"])\n\n    def test_display(self):\n        self.assertEqual(display_name(11), "User11")\n'},
    "reference": {"app/accounts.py": ACCOUNTS_CACHED, "tests/test_cache.py": TEST_CACHE_ACCOUNTS},
})

# ---------------------------------------------------------------- E2 signature change
TRANSPORT = 'FAIL = {"remaining": 0, "status": 503}\nCALLS = {"n": 0}\n\n\ndef send(method, url, timeout=None, headers=None):\n    """Fake transport. Set FAIL["remaining"] to inject errors."""\n    CALLS["n"] += 1\n    if FAIL["remaining"] > 0:\n        FAIL["remaining"] -= 1\n        return {"status": FAIL["status"], "body": None}\n    return {"status": 200, "body": {"url": url, "timeout": timeout, "headers": headers or {}}}\n'
HTTP_BASE = 'from app import transport\n\n\ndef get_json(url):\n    resp = transport.send("GET", url)\n    if resp["status"] != 200:\n        raise IOError(f"HTTP {resp[\'status\']}")\n    return resp["body"]\n'
HTTP_RETRY = 'from app import transport\n\n\ndef get_json(url, retries=3):\n    """GET url; retry up to `retries` times on 5xx responses."""\n    for _ in range(retries):\n        resp = transport.send("GET", url)\n        if resp["status"] < 500:\n            break\n    if resp["status"] != 200:\n        raise IOError(f"HTTP {resp[\'status\']}")\n    return resp["body"]\n'
TEST_RETRY = 'import unittest\nfrom app import transport\nfrom app.http import get_json\n\n\nclass RetryTest(unittest.TestCase):\n    def setUp(self):\n        transport.FAIL.update(remaining=0, status=503)\n\n    def test_retries_transient_errors(self):\n        transport.FAIL["remaining"] = 2\n        before = transport.CALLS["n"]\n        self.assertEqual(get_json("http://x/a")["url"], "http://x/a")\n        self.assertEqual(transport.CALLS["n"] - before, 3)\n\n    def test_gives_up_after_three(self):\n        transport.FAIL["remaining"] = 5\n        with self.assertRaises(IOError):\n            get_json("http://x/a")\n'
HTTP_CLIENT = 'from app import transport\n\n\nclass Client:\n    """HTTP client bound to a base URL with a default timeout."""\n\n    def __init__(self, base_url, timeout=5):\n        self.base_url = base_url.rstrip("/")\n        self.timeout = timeout\n\n    def get_json(self, path, headers=None):\n        resp = transport.send("GET", self.base_url + path, timeout=self.timeout, headers=headers)\n        if resp["status"] != 200:\n            raise IOError(f"HTTP {resp[\'status\']}")\n        return resp["body"]\n'
TEST_CLIENT = 'import unittest\nfrom app.http import Client\n\n\nclass ClientTest(unittest.TestCase):\n    def test_timeout_and_headers(self):\n        body = Client("http://x/", timeout=2).get_json("/a", headers={"k": "v"})\n        self.assertEqual(body, {"url": "http://x/a", "timeout": 2, "headers": {"k": "v"}})\n'
HTTP_CLIENT_RETRY = 'from app import transport\n\n\nclass Client:\n    """HTTP client bound to a base URL with a default timeout."""\n\n    def __init__(self, base_url, timeout=5, retries=3):\n        self.base_url = base_url.rstrip("/")\n        self.timeout = timeout\n        self.retries = retries\n\n    def get_json(self, path, headers=None):\n        for _ in range(self.retries):\n            resp = transport.send("GET", self.base_url + path, timeout=self.timeout, headers=headers)\n            if resp["status"] < 500:\n                break\n        if resp["status"] != 200:\n            raise IOError(f"HTTP {resp[\'status\']}")\n        return resp["body"]\n'
TEST_RETRY_CLIENT = 'import unittest\nfrom app import transport\nfrom app.http import Client\n\n\nclass RetryTest(unittest.TestCase):\n    def setUp(self):\n        transport.FAIL.update(remaining=0, status=503)\n\n    def test_retries_transient_errors(self):\n        transport.FAIL["remaining"] = 2\n        before = transport.CALLS["n"]\n        self.assertEqual(Client("http://x").get_json("/a")["url"], "http://x/a")\n        self.assertEqual(transport.CALLS["n"] - before, 3)\n\n    def test_gives_up_after_three(self):\n        transport.FAIL["remaining"] = 5\n        with self.assertRaises(IOError):\n            Client("http://x").get_json("/a")\n'
CASES.append({
    "id": "E2-pick-after-signature-change",
    "description": "Cherry-pick retry-on-5xx onto a branch where the get_json(url) function became a Client class with timeout/headers (git: content conflict).",
    "kind": "recovery",
    "base": {"app/__init__.py": "", "tests/__init__.py": "", ".gitignore": "__pycache__/\n",
             "app/transport.py": TRANSPORT, "app/http.py": HTTP_BASE,
             "tests/test_http.py": 'import unittest\nfrom app.http import get_json\n\n\nclass HttpTest(unittest.TestCase):\n    def test_ok(self):\n        self.assertEqual(get_json("http://x/a")["url"], "http://x/a")\n'},
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: retry GET requests on transient 5xx errors\n\nRetry up to 3 attempts in total; 4xx errors are not retried.",
         "files": {"app/http.py": HTTP_RETRY, "tests/test_retry.py": TEST_RETRY}},
        {"branch": "main", "message": "refactor: replace get_json() with a Client class (base URL, timeout, headers)",
         "files": {"app/http.py": HTTP_CLIENT, "tests/test_http.py": TEST_CLIENT}},
    ],
    "operation": {"op": "cherry-pick", "rev": "feature"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import transport\nfrom app.http import Client\n\n\nclass Hidden(unittest.TestCase):\n    def setUp(self):\n        transport.FAIL.update(remaining=0, status=503)\n\n    def test_recovers_after_two_5xx(self):\n        transport.FAIL["remaining"] = 2\n        b = transport.CALLS["n"]\n        body = Client("http://h", timeout=3).get_json("/p", headers={"a": "b"})\n        self.assertEqual(body, {"url": "http://h/p", "timeout": 3, "headers": {"a": "b"}})\n        self.assertEqual(transport.CALLS["n"] - b, 3)\n\n    def test_three_attempts_then_error(self):\n        transport.FAIL["remaining"] = 10\n        b = transport.CALLS["n"]\n        with self.assertRaises(IOError):\n            Client("http://h").get_json("/p")\n        self.assertEqual(transport.CALLS["n"] - b, 3)\n\n    def test_4xx_not_retried(self):\n        transport.FAIL.update(remaining=1, status=404)\n        b = transport.CALLS["n"]\n        with self.assertRaises(IOError):\n            Client("http://h").get_json("/p")\n        self.assertEqual(transport.CALLS["n"] - b, 1)\n'},
    "reference": {"app/http.py": HTTP_CLIENT_RETRY, "tests/test_retry.py": TEST_RETRY_CLIENT},
})

# ---------------------------------------------------------------- E3 clean but wrong
PRICING = 'def shipping(items):\n    """Flat shipping, in dollars."""\n    return 5\n\n\ndef subtotal(items):\n    """Sum of price * quantity, in dollars."""\n    return sum(price * qty for price, qty in items)\n\n\ndef item_count(items):\n    return sum(qty for _, qty in items)\n\n\ndef total(items):\n    return subtotal(items) + shipping(items)\n'
PRICING_DISCOUNT = PRICING.replace("def total(items):\n    return subtotal(items) + shipping(items)\n", "def total(items):\n    sub = subtotal(items)\n    if sub > 100:\n        sub = sub * 0.9\n    return sub + shipping(items)\n")
PRICING_CENTS = PRICING.replace("in dollars", "in cents").replace("return 5\n", "return 500\n").replace("sum(price * qty for price, qty in items)", "sum(round(price * 100) * qty for price, qty in items)")
PRICING_CENTS_DISCOUNT = PRICING_CENTS.replace("def total(items):\n    return subtotal(items) + shipping(items)\n", "def total(items):\n    sub = subtotal(items)\n    if sub > 100_00:\n        sub = round(sub * 0.9)\n    return sub + shipping(items)\n")
CASES.append({
    "id": "E3-pick-clean-but-wrong",
    "description": "Cherry-pick a '10% off orders over $100' discount onto a branch that switched all money to integer cents. Git applies cleanly but the threshold is now wrong.",
    "kind": "recovery",
    "base": {"app/__init__.py": "", "tests/__init__.py": "", ".gitignore": "__pycache__/\n", "app/pricing.py": PRICING,
             "tests/test_pricing.py": 'import unittest\nfrom app.pricing import total\n\n\nclass PricingTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([(1.5, 2)]), 8)\n'},
    "history": [
        {"branch": "discount", "from": "main", "message": "feat: 10% discount on orders whose subtotal exceeds $100",
         "files": {"app/pricing.py": PRICING_DISCOUNT,
                   "tests/test_discount.py": 'import unittest\nfrom app.pricing import total\n\n\nclass DiscountTest(unittest.TestCase):\n    def test_discount_over_100(self):\n        self.assertEqual(total([(60, 2)]), 113)\n'}},
        {"branch": "main", "message": "refactor: represent all money as integer cents",
         "files": {"app/pricing.py": PRICING_CENTS,
                   "tests/test_pricing.py": 'import unittest\nfrom app.pricing import total\n\n\nclass PricingTest(unittest.TestCase):\n    def test_total(self):\n        self.assertEqual(total([(1.5, 2)]), 800)\n'}},
    ],
    "operation": {"op": "cherry-pick", "rev": "discount"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app.pricing import total\n\n\nclass Hidden(unittest.TestCase):\n    def test_discount_applies_over_100_dollars(self):\n        self.assertEqual(total([(60, 2)]), 11300)\n\n    def test_no_discount_small_order(self):\n        self.assertEqual(total([(1.5, 2)]), 800)\n\n    def test_no_discount_at_exactly_100(self):\n        self.assertEqual(total([(100, 1)]), 10500)\n'},
    "reference": {"app/pricing.py": PRICING_CENTS_DISCOUNT,
                  "tests/test_discount.py": 'import unittest\nfrom app.pricing import total\n\n\nclass DiscountTest(unittest.TestCase):\n    def test_discount_over_100(self):\n        self.assertEqual(total([(60, 2)]), 113_00)\n'},
})

# ---------------------------------------------------------------- E4 merge
GREET = 'def greet(name):\n    return "Hello, " + name\n'
CASES.append({
    "id": "E4-merge-both-intentions",
    "description": "Merge a branch adding an optional punctuation argument into a branch that title-cases and trims names; both edit the same line.",
    "kind": "recovery",
    "base": {"app/__init__.py": "", "tests/__init__.py": "", ".gitignore": "__pycache__/\n", "app/greet.py": GREET,
             "tests/test_greet.py": 'import unittest\nfrom app.greet import greet\n\n\nclass GreetTest(unittest.TestCase):\n    def test_greet(self):\n        self.assertEqual(greet("Bob"), "Hello, Bob")\n'},
    "history": [
        {"branch": "punct", "from": "main", "message": "feat: optional punctuation argument for greet()",
         "files": {"app/greet.py": 'def greet(name, punct=""):\n    return "Hello, " + name + punct\n',
                   "tests/test_punct.py": 'import unittest\nfrom app.greet import greet\n\n\nclass PunctTest(unittest.TestCase):\n    def test_punct(self):\n        self.assertEqual(greet("Bob", "?"), "Hello, Bob?")\n'}},
        {"branch": "main", "message": "feat: trim and title-case names in greetings",
         "files": {"app/greet.py": 'def greet(name):\n    return "Hello, " + name.strip().title()\n',
                   "tests/test_title.py": 'import unittest\nfrom app.greet import greet\n\n\nclass TitleTest(unittest.TestCase):\n    def test_title_case(self):\n        self.assertEqual(greet(" ada lovelace "), "Hello, Ada Lovelace")\n'}},
    ],
    "operation": {"op": "merge", "rev": "punct"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app.greet import greet\n\n\nclass Hidden(unittest.TestCase):\n    def test_both(self):\n        self.assertEqual(greet(" ada lovelace ", "!"), "Hello, Ada Lovelace!")\n\n    def test_default(self):\n        self.assertEqual(greet("bob"), "Hello, Bob")\n\n    def test_punct_only(self):\n        self.assertEqual(greet("Bob", "?"), "Hello, Bob?")\n'},
    "reference": {"app/greet.py": 'def greet(name, punct=""):\n    return "Hello, " + name.strip().title() + punct\n'},
})

# ---------------------------------------------------------------- E5 revert after refactor
ACCOUNTS_MANY = ACCOUNTS_CACHED + '\n\ndef fetch_many(user_ids):\n    """Fetch several users, preserving order."""\n    return [fetch_user(u) for u in user_ids]\n'
CASES.append({
    "id": "E5-revert-after-refactor",
    "description": "Revert the caching feature after later commits moved the code and built fetch_many() on top of it (git: revert conflicts).",
    "kind": "recovery",
    "base": USERS_BASE,
    "history": [
        {"branch": "main", "tag": "cache", "message": "feat: cache user lookups",
         "files": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE}},
        {"branch": "main", "message": "refactor: rename get_user to fetch_user and move to app/accounts.py",
         "files": {"app/users.py": None, "app/accounts.py": ACCOUNTS_CACHED, "tests/test_users.py": TEST_ACCOUNTS, "tests/test_cache.py": TEST_CACHE_ACCOUNTS}},
        {"branch": "main", "message": "feat: fetch_many() for batch lookups",
         "files": {"app/accounts.py": ACCOUNTS_MANY,
                   "tests/test_many.py": 'import unittest\nfrom app.accounts import fetch_many\n\n\nclass ManyTest(unittest.TestCase):\n    def test_fetch_many_order(self):\n        self.assertEqual([u["id"] for u in fetch_many([3, 1, 2])], [3, 1, 2])\n'}},
    ],
    "operation": {"op": "revert", "rev": "cache"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import db\nfrom app.accounts import fetch_user, fetch_many, display_name\n\n\nclass Hidden(unittest.TestCase):\n    def test_no_longer_cached(self):\n        b = db.CALLS["n"]\n        fetch_user(7)\n        fetch_user(7)\n        self.assertEqual(db.CALLS["n"] - b, 2)\n\n    def test_fetch_many_still_works(self):\n        self.assertEqual([u["id"] for u in fetch_many([5, 4])], [5, 4])\n\n    def test_active_default_kept(self):\n        self.assertTrue(fetch_user(10)["active"])\n\n    def test_display(self):\n        self.assertEqual(display_name(11), "User11")\n'},
    "reference": {"app/accounts.py": ACCOUNTS + '\n\ndef fetch_many(user_ids):\n    """Fetch several users, preserving order."""\n    return [fetch_user(u) for u in user_ids]\n', "tests/test_cache.py": None},
})
