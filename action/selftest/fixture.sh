#!/usr/bin/env bash
# Build a PR-shaped repository for the action self-test.
#   fixture.sh <dir> good|stale|half|delete-test|no-marker
# Original change: caching on `dev`. Target `main` renamed get_user -> fetch_user
# (app/users.py -> app/accounts.py). PR branch `backport` is checked out at the end.
set -euo pipefail
dir="$1"; scenario="$2"
rm -rf "$dir"; mkdir -p "$dir"; cd "$dir"
git init -q -b main
git config user.email selftest@example.com; git config user.name selftest
mkdir -p app tests; : > app/__init__.py; : > tests/__init__.py
printf '__pycache__/\n' > .gitignore
cat > app/db.py <<'EOF'
CALLS = {"n": 0}


def load_user(uid):
    CALLS["n"] += 1
    return {"id": uid, "name": f"user{uid}"}
EOF
cat > app/users.py <<'EOF'
from app import db


def get_user(uid):
    return db.load_user(uid)


def display_name(uid):
    return get_user(uid)["name"].title()
EOF
cat > tests/test_users.py <<'EOF'
import unittest
from app.users import get_user, display_name


class UsersTest(unittest.TestCase):
    def test_get_user(self):
        self.assertEqual(get_user(1)["name"], "user1")

    def test_display_name(self):
        self.assertEqual(display_name(2), "User2")
EOF
git add -A; git commit -qm base

git checkout -qb dev
cat > app/users.py <<'EOF'
from app import db

_CACHE = {}


def get_user(uid):
    if uid not in _CACHE:
        _CACHE[uid] = db.load_user(uid)
    return _CACHE[uid]


def display_name(uid):
    return get_user(uid)["name"].title()
EOF
cat > tests/test_cache.py <<'EOF'
import unittest
from app import db
from app.users import get_user


class CacheTest(unittest.TestCase):
    def test_cache_hit(self):
        before = db.CALLS["n"]
        get_user(42)
        get_user(42)
        self.assertEqual(db.CALLS["n"] - before, 1)
EOF
git add -A; git commit -qm "feat: cache user lookups"
src=$(git rev-parse HEAD)

git checkout -q main
git rm -q app/users.py
cat > app/accounts.py <<'EOF'
from app import db


def fetch_user(user_id):
    record = db.load_user(user_id)
    record.setdefault("active", True)
    return record


def display_name(user_id):
    return fetch_user(user_id)["name"].title()
EOF
cat > tests/test_users.py <<'EOF'
import unittest
from app.accounts import fetch_user, display_name


class UsersTest(unittest.TestCase):
    def test_get_user(self):
        self.assertEqual(fetch_user(1)["name"], "user1")

    def test_display_name(self):
        self.assertEqual(display_name(2), "User2")

    def test_active_default(self):
        self.assertTrue(fetch_user(3)["active"])
EOF
git add -A; git commit -qm "refactor: rename get_user to fetch_user"

git checkout -qb backport
write_cached_accounts() {
cat > app/accounts.py <<'EOF'
from app import db

_CACHE = {}


def fetch_user(user_id):
    if user_id not in _CACHE:
        record = db.load_user(user_id)
        record.setdefault("active", True)
        _CACHE[user_id] = record
    return _CACHE[user_id]


def display_name(user_id):
    return fetch_user(user_id)["name"].title()
EOF
}
write_ported_test() { sed 's/from app.users import get_user/from app.accounts import fetch_user/; s/get_user(42)/fetch_user(42)/' > tests/test_cache.py <<'EOF'
import unittest
from app import db
from app.users import get_user


class CacheTest(unittest.TestCase):
    def test_cache_hit(self):
        before = db.CALLS["n"]
        get_user(42)
        get_user(42)
        self.assertEqual(db.CALLS["n"] - before, 1)
EOF
}
msg="Cache user lookups

(cherry picked from commit $src)"
case "$scenario" in
  good)        write_cached_accounts; write_ported_test ;;
  stale)       git checkout "$src" -- app/users.py tests/test_cache.py ;;
  half)        write_ported_test ;;
  delete-test) write_cached_accounts; write_ported_test
               sed -i '/def test_active_default/,$d' tests/test_users.py ;;
  no-marker)   printf 'notes\n' > NOTES.md; msg="unrelated change" ;;
  *) echo "unknown scenario $scenario" >&2; exit 2 ;;
esac
git add -A; git commit -qm "$msg"
echo "fixture $scenario ready in $dir (source $src)"
