//! Language-agnostic heuristics over unified diffs and file trees:
//! symbol extraction, test identifiers, symbol location search.

use crate::model::Symbol;
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn def_patterns() -> &'static [(Regex, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            (Regex::new(r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)").unwrap(), "function"),
            (Regex::new(r"^\s*class\s+([A-Za-z_]\w*)").unwrap(), "class"),
            (Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_]\w*)").unwrap(), "function"),
            (Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait)\s+([A-Za-z_]\w*)").unwrap(), "type"),
            (Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s+([A-Za-z_$][\w$]*)").unwrap(), "function"),
            (Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*=>").unwrap(), "function"),
            (Regex::new(r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)").unwrap(), "function"),
        ]
    })
}

fn test_patterns() -> &'static [Regex] {
    static P: OnceLock<Vec<Regex>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            Regex::new(r"^\s*(?:async\s+)?def\s+(test\w*)\s*\(").unwrap(),
            Regex::new(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(test_\w+)\s*\(").unwrap(),
            Regex::new(r"^\s*func\s+(Test\w+)\s*\(").unwrap(),
            Regex::new(r#"^\s*(?:it|test)\s*\(\s*['"`]([^'"`]+)['"`]"#).unwrap(),
        ]
    })
}

/// Definitions (name, kind) found on a single line of code.
pub fn defs_in_line(line: &str) -> Option<(String, &'static str)> {
    for (re, kind) in def_patterns() {
        if let Some(c) = re.captures(line) {
            return Some((c[1].to_string(), kind));
        }
    }
    None
}

pub fn test_in_line(line: &str) -> Option<String> {
    for re in test_patterns() {
        if let Some(c) = re.captures(line) {
            return Some(c[1].to_string());
        }
    }
    None
}

/// Iterate (file, origin, content) over a unified diff, origin in {'+','-',' ','@'}.
fn walk_patch(patch: &str, mut f: impl FnMut(&str, char, &str)) {
    let mut file = String::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let p = rest.trim();
            if p != "/dev/null" {
                file = p.strip_prefix("b/").unwrap_or(p).to_string();
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("--- ") {
            let p = rest.trim();
            if p != "/dev/null" {
                file = p.strip_prefix("a/").unwrap_or(p).to_string();
            }
            continue;
        }
        if line.starts_with("diff --git") || line.starts_with("index ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            // "@@ -a,b +c,d @@ context"
            let ctx = rest.split_once("@@").map(|x| x.1).unwrap_or("").trim();
            f(&file, '@', ctx);
            continue;
        }
        let mut chars = line.chars();
        if let Some(o) = chars.next() {
            if matches!(o, '+' | '-' | ' ') {
                f(&file, o, chars.as_str());
            }
        }
    }
}

/// Symbols defined in changed lines or enclosing changed hunks.
pub fn symbols_from_patch(patch: &str) -> Vec<Symbol> {
    let mut seen = BTreeSet::new();
    let mut out = vec![];
    // Most recent definition seen in the current file (context, hunk header or changed line):
    // a changed line is attributed to its enclosing definition.
    let mut enclosing: Option<(String, String, &'static str)> = None;
    let mut push = |out: &mut Vec<Symbol>, file: &str, name: &str, kind: &str| {
        if seen.insert((file.to_string(), name.to_string())) {
            out.push(Symbol {
                name: name.into(),
                kind: kind.into(),
                file: file.into(),
                verified: true,
            });
        }
    };
    walk_patch(patch, |file, origin, text| {
        if enclosing
            .as_ref()
            .map(|(f, _, _)| f != file)
            .unwrap_or(false)
        {
            enclosing = None;
        }
        let def = defs_in_line(text);
        if let Some((name, kind)) = &def {
            enclosing = Some((file.to_string(), name.clone(), kind));
        }
        if origin == '+' || origin == '-' {
            match def {
                Some((name, kind)) => push(&mut out, file, &name, kind),
                None => {
                    // Top-level statements (no indentation) are not inside the previous definition.
                    let indented = text.starts_with(' ') || text.starts_with('\t');
                    if let (true, Some((_, name, kind))) = (indented, &enclosing) {
                        push(&mut out, file, name, kind);
                    }
                }
            }
        }
    });
    out
}

/// Test identifiers introduced (added lines) by a patch.
pub fn tests_added_in_patch(patch: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    walk_patch(patch, |_f, origin, text| {
        if origin == '+' {
            if let Some(t) = test_in_line(text) {
                set.insert(t);
            }
        }
    });
    set.into_iter().collect()
}

/// Test identifiers removed by a patch.
pub fn tests_removed_in_patch(patch: &str) -> Vec<String> {
    let mut added = BTreeSet::new();
    let mut removed = BTreeSet::new();
    walk_patch(patch, |_f, origin, text| {
        if let Some(t) = test_in_line(text) {
            if origin == '+' {
                added.insert(t);
            } else if origin == '-' {
                removed.insert(t);
            }
        }
    });
    removed.difference(&added).cloned().collect()
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "__pycache__",
    ".venv",
    "venv",
    "dist",
    "build",
    ".gitent",
];

/// Text source files under `root` (relative paths), skipping VCS/build dirs.
pub fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(p);
                }
            } else if ft.is_file() && e.metadata().map(|m| m.len() < 1_000_000).unwrap_or(false) {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
    out.sort();
    out
}

/// All test identifiers defined in files under `root`.
pub fn tests_in_tree(root: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for rel in source_files(root) {
        if let Ok(text) = std::fs::read_to_string(root.join(&rel)) {
            for line in text.lines() {
                if let Some(t) = test_in_line(line) {
                    set.insert(t);
                }
            }
        }
    }
    set
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Location {
    pub symbol: String,
    pub file: String,
    pub line: usize,
    pub text: String,
    pub is_definition: bool,
}

/// Where do these names occur in the tree under `root`? Definitions first.
pub fn locate(root: &Path, names: &[String], limit: usize) -> Vec<Location> {
    let res: Vec<(String, Regex)> = names
        .iter()
        .filter(|n| n.len() >= 3)
        .filter_map(|n| {
            Regex::new(&format!(r"\b{}\b", regex::escape(n)))
                .ok()
                .map(|r| (n.clone(), r))
        })
        .collect();
    let mut out = vec![];
    for rel in source_files(root) {
        let Ok(text) = std::fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            for (n, re) in &res {
                if re.is_match(line) {
                    let is_def = defs_in_line(line).map(|(d, _)| &d == n).unwrap_or(false);
                    out.push(Location {
                        symbol: n.clone(),
                        file: rel.to_string_lossy().to_string(),
                        line: i + 1,
                        text: line.trim().chars().take(160).collect(),
                        is_definition: is_def,
                    });
                }
            }
        }
    }
    out.sort_by_key(|l| (!l.is_definition, l.file.clone(), l.line));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/app/users.py b/app/users.py
--- a/app/users.py
+++ b/app/users.py
@@ -1,3 +1,8 @@ import os
+_CACHE = {}
+
 def get_user(uid):
-    return db.load(uid)
+    if uid in _CACHE:
+        return _CACHE[uid]
+    return db.load(uid)
diff --git a/tests/test_users.py b/tests/test_users.py
--- /dev/null
+++ b/tests/test_users.py
@@ -0,0 +1,3 @@
+class TestCache(unittest.TestCase):
+    def test_cache_hit(self):
+        pass
";

    #[test]
    fn extracts_symbols() {
        let s = symbols_from_patch(PATCH);
        let names: Vec<_> = s.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"TestCache"), "{names:?}");
        assert!(names.contains(&"test_cache_hit"));
        assert!(
            names.contains(&"get_user"),
            "enclosing function of changed lines: {names:?}"
        );
        assert_eq!(
            s.iter().find(|s| s.name == "TestCache").unwrap().file,
            "tests/test_users.py"
        );
    }

    #[test]
    fn extracts_tests() {
        assert_eq!(
            tests_added_in_patch(PATCH),
            vec!["test_cache_hit".to_string()]
        );
        assert!(tests_removed_in_patch(PATCH).is_empty());
    }

    #[test]
    fn multi_language_defs() {
        assert_eq!(defs_in_line("pub fn load(x: u8) {").unwrap().0, "load");
        assert_eq!(
            defs_in_line("export async function fetchUser(id) {")
                .unwrap()
                .0,
            "fetchUser"
        );
        assert_eq!(
            defs_in_line("const add = (a, b) => a + b").unwrap().0,
            "add"
        );
        assert_eq!(
            defs_in_line("func (s *Srv) Handle(w http.ResponseWriter) {")
                .unwrap()
                .0,
            "Handle"
        );
        assert_eq!(
            test_in_line("  it('caches users', () => {").unwrap(),
            "caches users"
        );
        assert_eq!(
            test_in_line("func TestCache(t *testing.T) {").unwrap(),
            "TestCache"
        );
    }

    #[test]
    fn locate_finds_definitions_first() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.py"), "x = fetch_user(1)\n").unwrap();
        std::fs::write(
            d.path().join("b.py"),
            "\n\ndef fetch_user(uid):\n    pass\n",
        )
        .unwrap();
        let l = locate(d.path(), &["fetch_user".into()], 10);
        assert_eq!(l.len(), 2);
        assert!(l[0].is_definition);
        assert_eq!(l[0].file, "b.py");
        assert_eq!(l[0].line, 3);
    }
}
