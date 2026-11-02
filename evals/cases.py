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

# ---------------------------------------------------------------- E6 rebase a stack
USERS_TTL = 'from app import db, clock\n\n_CACHE = {}\nTTL = 60\n\n\ndef get_user(uid):\n    """Return the user, cached for TTL seconds."""\n    hit = _CACHE.get(uid)\n    if hit is None or clock.now() - hit[1] >= TTL:\n        hit = (db.load_user(uid), clock.now())\n        _CACHE[uid] = hit\n    return hit[0]\n\n\ndef display_name(uid):\n    return get_user(uid)["name"].title()\n'
USERS_STATS = USERS_TTL.replace('_CACHE = {}\nTTL = 60\n', '_CACHE = {}\nTTL = 60\nSTATS = {"hits": 0, "misses": 0}\n').replace(
    '    if hit is None or clock.now() - hit[1] >= TTL:\n        hit = (db.load_user(uid), clock.now())\n        _CACHE[uid] = hit\n',
    '    if hit is None or clock.now() - hit[1] >= TTL:\n        STATS["misses"] += 1\n        hit = (db.load_user(uid), clock.now())\n        _CACHE[uid] = hit\n    else:\n        STATS["hits"] += 1\n') + '\n\ndef cache_stats():\n    return dict(STATS)\n'
CLOCK = 'T = {"now": 1000.0}\n\n\ndef now():\n    return T["now"]\n'
TEST_TTL = 'import unittest\nfrom app import db, clock\nfrom app.users import get_user\n\n\nclass TtlTest(unittest.TestCase):\n    def test_entry_expires_after_ttl(self):\n        clock.T["now"] = 5000.0\n        b = db.CALLS["n"]\n        get_user(77)\n        clock.T["now"] += 61\n        get_user(77)\n        self.assertEqual(db.CALLS["n"] - b, 2)\n'
TEST_STATS = 'import unittest\nfrom app.users import get_user, cache_stats\n\n\nclass StatsTest(unittest.TestCase):\n    def test_hits_and_misses_counted(self):\n        s0 = cache_stats()\n        get_user(901)\n        get_user(901)\n        s1 = cache_stats()\n        self.assertEqual(s1["misses"] - s0["misses"], 1)\n        self.assertEqual(s1["hits"] - s0["hits"], 1)\n'
ACC = lambda src: src.replace("from app import db, clock", "from app import db, clock").replace("def get_user(uid):", "def fetch_user(user_id: int) -> dict:").replace("_CACHE.get(uid)", "_CACHE.get(user_id)").replace("db.load_user(uid)", "db.load_user(user_id)").replace("_CACHE[uid] = hit", "_CACHE[user_id] = hit").replace("def display_name(uid):\n    return get_user(uid)", "def display_name(user_id: int) -> str:\n    return fetch_user(user_id)")
ACCOUNTS_STATS_REF = '"""Account lookups (formerly app/users.py)."""\nfrom app import db, clock\n\n_CACHE = {}\nTTL = 60\nSTATS = {"hits": 0, "misses": 0}\n\n\ndef fetch_user(user_id: int) -> dict:\n    """Return the user record, cached for TTL seconds."""\n    hit = _CACHE.get(user_id)\n    if hit is None or clock.now() - hit[1] >= TTL:\n        STATS["misses"] += 1\n        record = db.load_user(user_id)\n        record.setdefault("active", True)\n        hit = (record, clock.now())\n        _CACHE[user_id] = hit\n    else:\n        STATS["hits"] += 1\n    return hit[0]\n\n\ndef display_name(user_id: int) -> str:\n    return fetch_user(user_id)["name"].title()\n\n\ndef cache_stats():\n    return dict(STATS)\n'
def _acc_tests(t):
    return t.replace("from app.users import get_user, cache_stats", "from app.accounts import fetch_user, cache_stats").replace("from app.users import get_user", "from app.accounts import fetch_user").replace("get_user(", "fetch_user(")
CASES.append({
    "id": "E6-rebase-stack-onto-refactor",
    "description": "Rebase a 3-commit feature stack (cache, TTL expiry, hit/miss stats) onto a main that renamed get_user to fetch_user and moved it to app/accounts.py.",
    "kind": "recovery",
    "base": dict(USERS_BASE, **{"app/clock.py": CLOCK}),
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: cache user lookups", "files": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE}},
        {"branch": "feature", "message": "feat: expire cached users after TTL (60s) using app.clock", "files": {"app/users.py": USERS_TTL, "tests/test_ttl.py": TEST_TTL}},
        {"branch": "feature", "message": "feat: count cache hits and misses; expose cache_stats()", "files": {"app/users.py": USERS_STATS, "tests/test_stats.py": TEST_STATS}},
        {"branch": "main", "message": "refactor: rename get_user to fetch_user and move to app/accounts.py",
         "files": {"app/users.py": None, "app/accounts.py": ACCOUNTS, "tests/test_users.py": TEST_ACCOUNTS}},
    ],
    "operation": {"op": "rebase", "branch": "feature", "rev": "main"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import db, clock\nfrom app.accounts import fetch_user, display_name, cache_stats\n\n\nclass Hidden(unittest.TestCase):\n    def test_cached_within_ttl(self):\n        clock.T["now"] = 10000.0\n        b = db.CALLS["n"]\n        fetch_user(501)\n        clock.T["now"] += 59\n        fetch_user(501)\n        self.assertEqual(db.CALLS["n"] - b, 1)\n\n    def test_expires_after_ttl(self):\n        clock.T["now"] = 20000.0\n        b = db.CALLS["n"]\n        fetch_user(502)\n        clock.T["now"] += 60\n        fetch_user(502)\n        self.assertEqual(db.CALLS["n"] - b, 2)\n\n    def test_stats(self):\n        s0 = cache_stats()\n        clock.T["now"] = 30000.0\n        fetch_user(503)\n        fetch_user(503)\n        fetch_user(503)\n        s1 = cache_stats()\n        self.assertEqual((s1["misses"] - s0["misses"], s1["hits"] - s0["hits"]), (1, 2))\n\n    def test_active_default_kept(self):\n        self.assertTrue(fetch_user(504)["active"])\n\n    def test_display(self):\n        self.assertEqual(display_name(505), "User505")\n'},
    "reference": {"app/accounts.py": ACCOUNTS_STATS_REF, "tests/test_cache.py": TEST_CACHE_ACCOUNTS,
                  "tests/test_ttl.py": _acc_tests(TEST_TTL), "tests/test_stats.py": _acc_tests(TEST_STATS)},
})

# ---------------------------------------------------------------- E7 target invariant trap
ACCOUNTS_COPY = '"""Account lookups."""\nfrom app import db\n\n\ndef fetch_user(user_id: int) -> dict:\n    """Return a fresh copy of the user record: callers may mutate it freely."""\n    record = dict(db.load_user(user_id))\n    record.setdefault("active", True)\n    return record\n\n\ndef display_name(user_id: int) -> str:\n    return fetch_user(user_id)["name"].title()\n'
TEST_COPY = TEST_ACCOUNTS + '\n    def test_callers_get_independent_copies(self):\n        a = fetch_user(4)\n        a["name"] = "mutated"\n        self.assertEqual(fetch_user(4)["name"], "user4")\n'
CASES.append({
    "id": "E7-pick-into-target-invariant",
    "description": "Cherry-pick caching onto a branch that (besides renaming) promises callers independent copies of user records. A naive cache returns the same dict and silently breaks that promise.",
    "kind": "recovery",
    "base": USERS_BASE,
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: cache user lookups\n\nLoad each user from the database at most once per process.",
         "files": {"app/users.py": USERS_CACHED, "tests/test_cache.py": TEST_CACHE}},
        {"branch": "main", "message": "refactor: rename get_user to fetch_user (app/accounts.py); fetch_user returns independent copies",
         "files": {"app/users.py": None, "app/accounts.py": ACCOUNTS_COPY, "tests/test_users.py": TEST_COPY}},
    ],
    "operation": {"op": "cherry-pick", "rev": "feature"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import db\nfrom app.accounts import fetch_user\n\n\nclass Hidden(unittest.TestCase):\n    def test_cached(self):\n        b = db.CALLS["n"]\n        fetch_user(61)\n        fetch_user(61)\n        self.assertEqual(db.CALLS["n"] - b, 1)\n\n    def test_mutation_does_not_leak_through_cache(self):\n        a = fetch_user(62)\n        a["name"] = "x"\n        a["active"] = False\n        b = fetch_user(62)\n        self.assertEqual((b["name"], b["active"]), ("user62", True))\n\n    def test_two_callers_independent(self):\n        self.assertIsNot(fetch_user(63), fetch_user(63))\n'},
    "reference": {"app/accounts.py": '"""Account lookups."""\nfrom app import db\n\n_CACHE = {}\n\n\ndef fetch_user(user_id: int) -> dict:\n    """Return a fresh copy of the (cached) user record: callers may mutate it freely."""\n    if user_id not in _CACHE:\n        record = dict(db.load_user(user_id))\n        record.setdefault("active", True)\n        _CACHE[user_id] = record\n    return dict(_CACHE[user_id])\n\n\ndef display_name(user_id: int) -> str:\n    return fetch_user(user_id)["name"].title()\n',
                  "tests/test_cache.py": TEST_CACHE_ACCOUNTS},
})

# ---------------------------------------------------------------- E8 multi-site intent
AUDIT = 'EVENTS = []\n\n\ndef record(event, order_id):\n    EVENTS.append((event, order_id))\n'
ORDERS = 'ORDERS = {}\n\n\ndef create_order(order_id, amount):\n    ORDERS[order_id] = {"amount": amount, "state": "open"}\n    return ORDERS[order_id]\n\n\ndef cancel_order(order_id):\n    ORDERS[order_id]["state"] = "cancelled"\n\n\ndef refund_order(order_id):\n    ORDERS[order_id]["state"] = "refunded"\n'
ORDERS_AUDITED = 'from app import audit\n\nORDERS = {}\n\n\ndef create_order(order_id, amount):\n    ORDERS[order_id] = {"amount": amount, "state": "open"}\n    audit.record("created", order_id)\n    return ORDERS[order_id]\n\n\ndef cancel_order(order_id):\n    ORDERS[order_id]["state"] = "cancelled"\n    audit.record("cancelled", order_id)\n\n\ndef refund_order(order_id):\n    ORDERS[order_id]["state"] = "refunded"\n    audit.record("refunded", order_id)\n'
TEST_AUDIT = 'import unittest\nfrom app import audit\nfrom app.orders import create_order, cancel_order, refund_order\n\n\nclass AuditTest(unittest.TestCase):\n    def test_every_transition_is_audited(self):\n        audit.EVENTS.clear()\n        create_order(1, 10)\n        cancel_order(1)\n        create_order(2, 20)\n        refund_order(2)\n        self.assertEqual(audit.EVENTS, [("created", 1), ("cancelled", 1), ("created", 2), ("refunded", 2)])\n'
ORDERS_MAIN = 'ORDERS = {}\n\n\ndef create_order(order_id, amount):\n    ORDERS[order_id] = {"amount": amount, "state": "open"}\n    return ORDERS[order_id]\n\n\ndef void_order(order_id):\n    """Formerly cancel_order."""\n    ORDERS[order_id]["state"] = "cancelled"\n\n\ndef ship_order(order_id):\n    ORDERS[order_id]["state"] = "shipped"\n'
REFUNDS_MAIN = 'from app.orders import ORDERS\n\n\ndef refund_order(order_id, reason=""):\n    ORDERS[order_id]["state"] = "refunded"\n    ORDERS[order_id]["refund_reason"] = reason\n'
CASES.append({
    "id": "E8-pick-multi-site-intent",
    "description": "Cherry-pick 'audit every state-changing order operation' onto a branch that renamed cancel_order to void_order, moved refunds to app/refunds.py, and added ship_order.",
    "kind": "recovery",
    "base": {"app/__init__.py": "", "tests/__init__.py": "", ".gitignore": "__pycache__/\n", "app/audit.py": AUDIT, "app/orders.py": ORDERS,
             "tests/test_orders.py": 'import unittest\nfrom app.orders import create_order\n\n\nclass OrdersTest(unittest.TestCase):\n    def test_create(self):\n        self.assertEqual(create_order(9, 5)["state"], "open")\n'},
    "history": [
        {"branch": "feature", "from": "main", "message": "feat: audit every state-changing order operation\n\nEvery operation that changes an order's state must call audit.record(<event>, order_id).",
         "files": {"app/orders.py": ORDERS_AUDITED, "tests/test_audit.py": TEST_AUDIT}},
        {"branch": "main", "message": "refactor: rename cancel_order to void_order, move refunds to app/refunds.py, add ship_order",
         "files": {"app/orders.py": ORDERS_MAIN, "app/refunds.py": REFUNDS_MAIN}},
    ],
    "operation": {"op": "cherry-pick", "rev": "feature"},
    "hidden": {"tests/test_hidden.py": 'import unittest\nfrom app import audit\nfrom app.orders import create_order, void_order, ship_order\nfrom app.refunds import refund_order\n\n\nclass Hidden(unittest.TestCase):\n    def setUp(self):\n        audit.EVENTS.clear()\n\n    def test_create_void_refund_audited(self):\n        create_order(1, 10)\n        void_order(1)\n        create_order(2, 20)\n        refund_order(2, reason="damaged")\n        names = [e for e, _ in audit.EVENTS]\n        self.assertEqual(len(audit.EVENTS), 4)\n        self.assertEqual([i for _, i in audit.EVENTS], [1, 1, 2, 2])\n        self.assertEqual(names[0], "created")\n\n    def test_ship_is_audited_too(self):\n        create_order(3, 30)\n        ship_order(3)\n        self.assertEqual([i for _, i in audit.EVENTS], [3, 3])\n\n    def test_refund_reason_kept(self):\n        from app.orders import ORDERS\n        create_order(4, 1)\n        refund_order(4, reason="late")\n        self.assertEqual(ORDERS[4]["refund_reason"], "late")\n'},
    "reference": {"app/orders.py": 'from app import audit\n\nORDERS = {}\n\n\ndef create_order(order_id, amount):\n    ORDERS[order_id] = {"amount": amount, "state": "open"}\n    audit.record("created", order_id)\n    return ORDERS[order_id]\n\n\ndef void_order(order_id):\n    """Formerly cancel_order."""\n    ORDERS[order_id]["state"] = "cancelled"\n    audit.record("cancelled", order_id)\n\n\ndef ship_order(order_id):\n    ORDERS[order_id]["state"] = "shipped"\n    audit.record("shipped", order_id)\n',
                  "app/refunds.py": 'from app import audit\nfrom app.orders import ORDERS\n\n\ndef refund_order(order_id, reason=""):\n    ORDERS[order_id]["state"] = "refunded"\n    ORDERS[order_id]["refund_reason"] = reason\n    audit.record("refunded", order_id)\n',
                  "tests/test_audit.py": 'import unittest\nfrom app import audit\nfrom app.orders import create_order, void_order\nfrom app.refunds import refund_order\n\n\nclass AuditTest(unittest.TestCase):\n    def test_every_transition_is_audited(self):\n        audit.EVENTS.clear()\n        create_order(1, 10)\n        void_order(1)\n        create_order(2, 20)\n        refund_order(2)\n        self.assertEqual(audit.EVENTS, [("created", 1), ("cancelled", 1), ("created", 2), ("refunded", 2)])\n'},
})
