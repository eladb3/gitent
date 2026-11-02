//! Shared helpers for integration tests: real git repositories, real Python
//! test suites, and real processes as stand-in agents (ADR 0007).
#![allow(dead_code)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct TestRepo {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    _tools: tempfile::TempDir,
    pub tools: PathBuf,
}

pub fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_gitent")
}

/// Run a command with a clean gitent environment.
fn cmd(program: &str, cwd: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(cwd);
    for (k, _) in std::env::vars() {
        if k.starts_with("GITENT_") {
            c.env_remove(k);
        }
    }
    c.env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    c
}

impl TestRepo {
    /// An empty git repository on branch `main` (no commits, no gitent config).
    pub fn empty() -> TestRepo {
        let dir = tempfile::tempdir().unwrap();
        let tools = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo");
        std::fs::create_dir_all(&path).unwrap();
        let r = TestRepo {
            path,
            tools: tools.path().to_path_buf(),
            _dir: dir,
            _tools: tools,
        };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.email", "test@example.com"]);
        r.git(&["config", "user.name", "Test"]);
        r
    }

    /// The users project (base commit made with plain git, gitent initialized).
    pub fn users() -> TestRepo {
        let r = TestRepo::empty();
        r.write("app/__init__.py", "");
        r.write("tests/__init__.py", "");
        r.write("app/db.py", DB_PY);
        r.write("app/users.py", USERS_PY);
        r.write("tests/test_users.py", TEST_USERS_PY);
        r.write(".gitignore", "__pycache__/\n");
        let (code, v) = r.gitent(&["init"]);
        assert_eq!(code, 0, "{v}");
        r.git(&["add", "-A"]);
        r.git(&["commit", "-q", "-m", "base: users service"]);
        r
    }

    pub fn write(&self, rel: &str, content: &str) {
        let p = self.path.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path.join(rel)).unwrap()
    }

    pub fn rm(&self, rel: &str) {
        std::fs::remove_file(self.path.join(rel)).unwrap();
    }

    pub fn git_try(&self, args: &[&str]) -> (bool, String) {
        let o = cmd("git", &self.path).args(args).output().unwrap();
        (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        )
    }

    pub fn git(&self, args: &[&str]) -> String {
        let (ok, out) = self.git_try(args);
        assert!(ok, "git {args:?} failed: {out}");
        out.trim().to_string()
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }

    pub fn rev(&self, r: &str) -> String {
        self.git(&["rev-parse", r])
    }

    /// Run `gitent --json <args>`; returns (exit code, parsed JSON).
    pub fn gitent(&self, args: &[&str]) -> (i32, Value) {
        self.gitent_env(args, &[])
    }

    pub fn gitent_env(&self, args: &[&str], env: &[(&str, &str)]) -> (i32, Value) {
        let mut c = cmd(bin(), &self.path);
        c.arg("--json").args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        let stdout = String::from_utf8_lossy(&o.stdout).to_string();
        let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|_| {
            panic!(
                "non-JSON output for {args:?}: {stdout}\nstderr: {}",
                String::from_utf8_lossy(&o.stderr)
            )
        });
        (o.status.code().unwrap_or(-1), v)
    }

    /// Human (non-JSON) output.
    pub fn gitent_text(&self, args: &[&str]) -> (i32, String) {
        let o = cmd(bin(), &self.path).args(args).output().unwrap();
        (
            o.status.code().unwrap_or(-1),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        )
    }

    /// Commit everything with gitent (no LLM); asserts success.
    pub fn commit(&self, msg: &str, extra: &[&str]) -> Value {
        let mut a = vec!["commit", "-m", msg];
        a.extend_from_slice(extra);
        let (code, v) = self.gitent(&a);
        assert_eq!(code, 0, "gitent commit failed: {v}");
        v
    }

    /// Write an executable shell script (a stand-in agent or LLM) outside the repo.
    pub fn script(&self, name: &str, body: &str) -> String {
        let p = self.tools.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nset -e\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p.to_string_lossy().to_string()
    }

    pub fn note(&self, rev: &str) -> Value {
        let n = self.git(&["notes", "--ref", "gitent", "show", rev]);
        serde_json::from_str(&n).unwrap()
    }

    pub fn worktrees(&self) -> usize {
        self.git(&["worktree", "list"]).lines().count()
    }

    pub fn local_config(&self, toml: &str) {
        let p = self.path.join(".git/gitent/config.toml");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, toml).unwrap();
    }
}

/// `cat > path <<EOF` for scripts.
pub fn heredoc(path: &str, content: &str) -> String {
    format!("mkdir -p \"$(dirname {path})\"\ncat > {path} <<'GITENT_EOF'\n{content}GITENT_EOF\n")
}

// ------------------------------------------------------------------ fixtures

pub const DB_PY: &str = r#"CALLS = {"n": 0}


def load_user(uid):
    CALLS["n"] += 1
    return {"id": uid, "name": f"user{uid}"}
"#;

pub const USERS_PY: &str = r#"from app import db


def get_user(uid):
    return db.load_user(uid)


def display_name(uid):
    return get_user(uid)["name"].title()
"#;

pub const TEST_USERS_PY: &str = r#"import unittest
from app.users import get_user, display_name


class UsersTest(unittest.TestCase):
    def test_get_user(self):
        self.assertEqual(get_user(1)["name"], "user1")

    def test_display_name(self):
        self.assertEqual(display_name(2), "User2")
"#;

pub const USERS_CACHED_PY: &str = r#"from app import db

_CACHE = {}


def get_user(uid):
    if uid not in _CACHE:
        _CACHE[uid] = db.load_user(uid)
    return _CACHE[uid]


def display_name(uid):
    return get_user(uid)["name"].title()
"#;

pub const TEST_CACHE_PY: &str = r#"import unittest
from app import db
from app.users import get_user


class CacheTest(unittest.TestCase):
    def test_cache_hit(self):
        before = db.CALLS["n"]
        get_user(42)
        get_user(42)
        self.assertEqual(db.CALLS["n"] - before, 1)
"#;

/// Target-branch refactor: users.py → accounts.py, get_user → fetch_user.
pub const ACCOUNTS_PY: &str = r#""""Account lookups (formerly app/users.py)."""
from app import db


def fetch_user(user_id: int) -> dict:
    """Return the user record for ``user_id``."""
    record = db.load_user(user_id)
    record.setdefault("active", True)
    return record


def display_name(user_id: int) -> str:
    return fetch_user(user_id)["name"].title()
"#;

pub const TEST_ACCOUNTS_PY: &str = r#"import unittest
from app.accounts import fetch_user, display_name


class UsersTest(unittest.TestCase):
    def test_get_user(self):
        self.assertEqual(fetch_user(1)["name"], "user1")

    def test_display_name(self):
        self.assertEqual(display_name(2), "User2")

    def test_active_default(self):
        self.assertTrue(fetch_user(3)["active"])
"#;

pub const ACCOUNTS_CACHED_PY: &str = r#""""Account lookups (formerly app/users.py)."""
from app import db

_CACHE = {}


def fetch_user(user_id: int) -> dict:
    """Return the user record for ``user_id`` (cached)."""
    if user_id not in _CACHE:
        record = db.load_user(user_id)
        record.setdefault("active", True)
        _CACHE[user_id] = record
    return _CACHE[user_id]


def display_name(user_id: int) -> str:
    return fetch_user(user_id)["name"].title()
"#;

pub const TEST_CACHE_ACCOUNTS_PY: &str = r#"import unittest
from app import db
from app.accounts import fetch_user


class CacheTest(unittest.TestCase):
    def test_cache_hit(self):
        before = db.CALLS["n"]
        fetch_user(42)
        fetch_user(42)
        self.assertEqual(db.CALLS["n"] - before, 1)
"#;

impl TestRepo {
    /// Branch `feature` (from base) with the caching intention committed by gitent.
    pub fn add_cache_feature(&self) -> String {
        self.git(&["checkout", "-q", "-b", "feature"]);
        self.write("app/users.py", USERS_CACHED_PY);
        self.write("tests/test_cache.py", TEST_CACHE_PY);
        self.commit("feat: cache user lookups", &[]);
        let h = self.head();
        self.git(&["checkout", "-q", "main"]);
        h
    }

    /// On the current branch: rename/move users.py → accounts.py (plain git commit).
    pub fn rename_on_current(&self) {
        self.git(&["rm", "-q", "app/users.py"]);
        self.write("app/accounts.py", ACCOUNTS_PY);
        self.write("tests/test_users.py", TEST_ACCOUNTS_PY);
        self.git(&["add", "-A"]);
        self.git(&[
            "commit",
            "-q",
            "-m",
            "refactor: rename get_user to fetch_user, move to accounts",
        ]);
    }

    /// A stand-in agent that re-implements caching on the renamed code.
    pub fn good_cache_agent(&self) -> String {
        self.script(
            "good_cache_agent.sh",
            &format!(
                "cat > /dev/null\n{}{}echo 'ported caching to fetch_user'",
                heredoc("app/accounts.py", ACCOUNTS_CACHED_PY),
                heredoc("tests/test_cache.py", TEST_CACHE_ACCOUNTS_PY)
            ),
        )
    }
}
