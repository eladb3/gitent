#!/usr/bin/env bash
# Reproducible terminal demo of Gitent (recorded into docs/demo/demo.gif).
#   asciinema rec -c "bash docs/demo/demo.sh" demo.cast
# Needs: gitent and python3 on PATH.
set -uo pipefail
B=$'\e[1m'; D=$'\e[2m'; C=$'\e[36m'; G=$'\e[32m'; Y=$'\e[33m'; M=$'\e[35m'; R=$'\e[0m'
SPEED="${DEMO_SPEED:-0.025}"

SCENE=0
say()   { SCENE=$((SCENE+1)); if [ "$SCENE" -gt 1 ]; then sleep 1.5; clear; fi
          printf "${C}${B}gitent${R}${D} demo · %s/5${R}\n\n${M}${B}▌ %s${R}\n" "$SCENE" "$*"; sleep 1.2; }
note()  { printf "${D}  %s${R}\n" "$*"; sleep 0.6; }
type_() { local s="$1" i; printf "${G}\$${R} "; for ((i=0;i<${#s};i++)); do printf '%s' "${s:i:1}"; sleep "$SPEED"; done; printf '\n'; sleep 0.35; }
run()   { type_ "$1"; eval "$1"; sleep "${2:-1.4}"; }
quiet() { "$@" >/dev/null 2>&1; }

# ---------------------------------------------------------------- off camera: a small project
WORK=$(mktemp -d); cd "$WORK"
export GIT_AUTHOR_NAME=dev GIT_AUTHOR_EMAIL=dev@example.com GIT_COMMITTER_NAME=dev GIT_COMMITTER_EMAIL=dev@example.com
export PYTHONDONTWRITEBYTECODE=1
quiet git init -q -b 1.x shop && cd shop
mkdir -p app tests; : > app/__init__.py; : > tests/__init__.py; printf '__pycache__/\n' > .gitignore
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
EOF
cat > tests/test_users.py <<'EOF'
import unittest
from app.users import get_user


class UsersTest(unittest.TestCase):
    def test_get_user(self):
        self.assertEqual(get_user(1)["name"], "user1")
EOF
quiet gitent init; quiet git add -A; quiet git commit -m "v1.0"
# main moved on: users.py was refactored into accounts.py (get_user -> fetch_user)
quiet git switch -c main
quiet git rm -q app/users.py
cat > app/accounts.py <<'EOF'
from app import db


def fetch_user(user_id):
    record = db.load_user(user_id)
    record.setdefault("active", True)
    return record
EOF
sed -i 's/from app.users import get_user/from app.accounts import fetch_user/; s/get_user(1)/fetch_user(1)/' tests/test_users.py
quiet git add -A; quiet git commit -m "refactor: users.py -> accounts.py, get_user -> fetch_user"
quiet git switch 1.x
# the fix, written on the maintenance branch
cat > app/users.py <<'EOF'
from app import db

_CACHE = {}


def get_user(uid):
    if uid not in _CACHE:
        _CACHE[uid] = db.load_user(uid)
    return _CACHE[uid]
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
clear

# ---------------------------------------------------------------- on camera

say "1. A bug fix on the 1.x maintenance branch: cache user lookups"
run "git status --short" 1
run "gitent commit -m 'fix: cache user lookups'" 3.5
note "The new test became a requirement, proven to FAIL before the fix and PASS after it."

say "2. Port the fix to main — where users.py was refactored into accounts.py"
run "git switch -q main"
run "git cherry-pick 1.x 2>&1 | grep CONFLICT" 1.5
run "git cherry-pick --abort"
note "Git can't do it: the file it patches no longer exists on main."

say "3. An agent ports it quickly... by copying the old file back"
run "git switch -q -c port-quick"
run "git checkout 1.x -- app/users.py tests/test_cache.py && git commit -qm 'port caching fix'"
run "python3 -m unittest -q 2>&1 | tail -1" 1.5
note "Tests pass, so the agent reports success. But main's code never got the cache."
run "gitent verify 1.x" 4.5

say "4. A proper port: cache fetch_user() in accounts.py, port the test"
run "git switch -q -c port-proper main"
quiet python3 - <<'EOF'
import re
p = "app/accounts.py"; s = open(p).read()
s = s.replace("from app import db\n", "from app import db\n\n_CACHE = {}\n", 1)
s = s.replace('''def fetch_user(user_id):
    record = db.load_user(user_id)
    record.setdefault("active", True)
    return record''', '''def fetch_user(user_id):
    if user_id not in _CACHE:
        record = db.load_user(user_id)
        record.setdefault("active", True)
        _CACHE[user_id] = record
    return _CACHE[user_id]''')
open(p, "w").write(s)
EOF
git show 1.x:tests/test_cache.py | sed 's/from app.users import get_user/from app.accounts import fetch_user/; s/get_user(42)/fetch_user(42)/' > tests/test_cache.py
run "git status --short" 1.5
run "git add -A && git commit -qm 'port caching fix to accounts.fetch_user'"
run "gitent verify 1.x --record" 4.5
run "gitent log -n 2" 3

say "In CI: every backport PR gets this check —  uses: eladb3/gitent@v0"
sleep 3
rm -rf "$WORK"
