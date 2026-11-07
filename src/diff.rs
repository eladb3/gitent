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
            // 0 python def, 1 class (py/js/ts/java/kotlin/swift/c#)
            (Regex::new(r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)").unwrap(), "function"),
            (Regex::new(r"^\s*(?:(?:export|public|private|protected|internal|abstract|final|sealed|static|open|data|default|partial)\s+)*class\s+([A-Za-z_]\w*)").unwrap(), "class"),
            // 2 rust fn, 3 rust type
            (Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+([A-Za-z_]\w*)").unwrap(), "function"),
            (Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait)\s+([A-Za-z_]\w*)").unwrap(), "type"),
            // 4 js function, 5 js/ts arrow (optionally typed)
            (Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s+([A-Za-z_$][\w$]*)").unwrap(), "function"),
            (Regex::new(r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::\s*[^=]+)?=\s*(?:async\s*)?(?:\([^)]*\)|[A-Za-z_$][\w$]*)\s*(?::\s*[^=]+?)?\s*=>").unwrap(), "function"),
            // 6 go func
            (Regex::new(r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)").unwrap(), "function"),
            // 7 java/c#/kotlin-style method with modifiers: `public static int total(`
            (Regex::new(r"^\s*(?:@\w+\s+)*(?:(?:public|private|protected|internal|static|final|abstract|synchronized|override|virtual|async|open|suspend)\s+)+(?:[\w<>\[\],.?]+\s+)?([A-Za-z_]\w*)\s*\(").unwrap(), "function"),
            // 8 kotlin/swift fun/func
            (Regex::new(r"^\s*(?:(?:public|private|internal|override|open|static|suspend)\s+)*(?:fun|func)\s+(?:<[^>]*>\s*)?([A-Za-z_]\w*)").unwrap(), "function"),
            // 9 ts class method: `  async total(items: Item[]): number {`
            (Regex::new(r"^\s+(?:(?:public|private|protected|static|async|readonly)\s+)*([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*(?::\s*[^{=]+)?\{\s*$").unwrap(), "function"),
        ]
    })
}

/// Keywords that look like a method header to pattern 9 (`if (x) {`).
const NOT_NAMES: &[&str] = &[
    "if", "for", "while", "switch", "catch", "return", "function", "else", "do", "try", "with",
];

fn test_patterns() -> &'static [Regex] {
    static P: OnceLock<Vec<Regex>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            Regex::new(r"^\s*(?:async\s+)?def\s+(test\w*)\s*\(").unwrap(),
            Regex::new(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(test_\w+)\s*\(").unwrap(),
            Regex::new(r"^\s*func\s+(Test\w+)\s*\(").unwrap(),
            Regex::new(r#"^\s*(?:it|test|specify)\s*(?:\(\s*|\s+)['"`]([^'"`]+)['"`]"#).unwrap(),
            // Swift XCTest
            Regex::new(r"^\s*(?:override\s+)?func\s+(test\w+)\s*\(").unwrap(),
        ]
    })
}

/// An attribute/annotation that marks the next function as a test.
fn test_attr() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^\s*(?:#\[(?:\w+::)*test\b[^\]]*\]|@(?:Test|ParameterizedTest|RepeatedTest|TestFactory)\b|\[(?:Test|Fact|Theory|TestMethod|TestCase)\b)").unwrap()
    })
}

/// A function/method header following a test attribute.
fn attr_fn() -> &'static [Regex] {
    static P: OnceLock<Vec<Regex>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fun|fn|func)\s+(`[^`]+`|[A-Za-z_]\w*)").unwrap(),
            Regex::new(r"^\s*(?:(?:public|private|protected|internal|static|final|async|override|virtual|suspend)\s+)*(?:[\w<>\[\],.?]+\s+)([A-Za-z_]\w*)\s*\(").unwrap(),
        ]
    })
}

/// Line-by-line test detection that also understands attribute-marked tests
/// (`#[test]`, `@Test`, `[Fact]`, … on a preceding line).
#[derive(Default)]
pub struct TestScan {
    pending: bool,
}

impl TestScan {
    pub fn feed(&mut self, line: &str) -> Option<String> {
        if test_attr().is_match(line) {
            self.pending = true;
            return None;
        }
        if self.pending {
            let t = line.trim_start();
            if t.is_empty()
                || t.starts_with('#')
                || t.starts_with('@')
                || t.starts_with('[')
                || t.starts_with("//")
            {
                return None;
            }
            self.pending = false;
            for re in attr_fn() {
                if let Some(c) = re.captures(line) {
                    return Some(c[1].trim_matches('`').to_string());
                }
            }
        }
        test_in_line(line)
    }
}

/// (test name, 0-based line index) for every test defined in `text`.
pub fn tests_in_text(text: &str) -> Vec<(String, usize)> {
    let mut sc = TestScan::default();
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| sc.feed(l).map(|t| (t, i)))
        .collect()
}

/// Which definition patterns apply to a file (indices into `def_patterns`).
/// Unknown extensions use all patterns.
fn patterns_for(path: &str) -> &'static [usize] {
    let ext = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "py" | "pyi" => &[0, 1],
        "rs" => &[2, 3],
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => &[1, 4, 5, 9],
        "go" => &[6],
        "java" | "cs" | "scala" => &[1, 7],
        "kt" | "kts" | "swift" => &[1, 7, 8],
        "c" | "h" | "cc" | "cpp" | "hpp" | "php" | "rb" => &[1, 4, 7],
        _ => &[0, 1, 2, 3, 4, 5, 6],
    }
}

/// Definitions on a line of the given file, using only that language's patterns
/// (a Python docstring saying "struct does not…" is not a Rust struct).
pub fn defs_in_line_for(path: &str, line: &str) -> Option<(String, &'static str)> {
    let pats = def_patterns();
    for &i in patterns_for(path) {
        let (re, kind) = &pats[i];
        if let Some(c) = re.captures(line) {
            if NOT_NAMES.contains(&&c[1]) {
                continue;
            }
            return Some((c[1].to_string(), kind));
        }
    }
    None
}

/// Definitions (name, kind) found on a single line of code (any language).
pub fn defs_in_line(line: &str) -> Option<(String, &'static str)> {
    for (re, kind) in def_patterns() {
        if let Some(c) = re.captures(line) {
            if NOT_NAMES.contains(&&c[1]) {
                continue;
            }
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
        let def = defs_in_line_for(file, text);
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

/// (path, test) pairs defined on one side of a patch: `new_side` = added lines
/// (with context), otherwise removed lines (with context).
fn tests_on_side(patch: &str, new_side: bool) -> BTreeSet<(String, String)> {
    let mine = if new_side { '+' } else { '-' };
    let mut set = BTreeSet::new();
    let mut sc = TestScan::default();
    let mut cur = String::new();
    walk_patch(patch, |file, origin, text| {
        if file != cur || origin == '@' {
            cur = file.to_string();
            sc = TestScan::default();
            if origin == '@' {
                return;
            }
        }
        if origin == ' ' || origin == mine {
            if let Some(t) = sc.feed(text) {
                if origin == mine {
                    set.insert((file.to_string(), t));
                }
            }
        }
    });
    set
}

/// Tests introduced (added lines) by a patch, with the file they are added in.
pub fn tests_added_with_paths(patch: &str) -> Vec<(String, String)> {
    tests_on_side(patch, true).into_iter().collect()
}

/// Test identifiers introduced (added lines) by a patch.
pub fn tests_added_in_patch(patch: &str) -> Vec<String> {
    let set: BTreeSet<String> = tests_on_side(patch, true)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    set.into_iter().collect()
}

/// Test identifiers removed by a patch.
pub fn tests_removed_in_patch(patch: &str) -> Vec<String> {
    let added: BTreeSet<String> = tests_on_side(patch, true)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    let removed: BTreeSet<String> = tests_on_side(patch, false)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
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
            for (t, _) in tests_in_text(&text) {
                set.insert(t);
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
                    let is_def = defs_in_line_for(&rel.to_string_lossy(), line)
                        .map(|(d, _)| &d == n)
                        .unwrap_or(false);
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
    fn defs_are_language_specific() {
        assert!(defs_in_line_for(
            "tests/i18n/tests.py",
            "        struct does not contain the 'name' key."
        )
        .is_none());
        assert_eq!(
            defs_in_line_for("a.py", "class Foo(Base):").unwrap().0,
            "Foo"
        );
        assert!(defs_in_line_for("a.rs", "def nope():").is_none());
        assert_eq!(
            defs_in_line_for("a.rs", "pub struct Repo {").unwrap().0,
            "Repo"
        );
        assert_eq!(
            defs_in_line_for("a.ts", "export function go() {")
                .unwrap()
                .0,
            "go"
        );
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
    fn attribute_marked_tests_and_more_languages() {
        let rs = "#[test]\nfn parses_config() {\n}\n#[tokio::test]\nasync fn serves() {}\nfn helper() {}\n";
        let names: Vec<String> = tests_in_text(rs).into_iter().map(|x| x.0).collect();
        assert_eq!(names, vec!["parses_config", "serves"]);
        let java =
            "class T {\n  @Test\n  public void totalIsClamped() {\n  }\n  void helper() {}\n}\n";
        let names: Vec<String> = tests_in_text(java).into_iter().map(|x| x.0).collect();
        assert_eq!(names, vec!["totalIsClamped"]);
        let cs = "[Fact]\npublic async Task Clamps() {}\n";
        assert_eq!(tests_in_text(cs)[0].0, "Clamps");
        let kt = "@Test\nfun `discount is clamped`() {}\n";
        assert_eq!(tests_in_text(kt)[0].0, "discount is clamped");
        assert_eq!(
            test_in_line("  it \"clamps the discount\" do").unwrap(),
            "clamps the discount"
        );
        assert_eq!(test_in_line("    func testClamp() {").unwrap(), "testClamp");
        // patches: attribute in context, fn added
        let p = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,4 @@\n #[test]\n+fn added_case() {\n+}\n";
        assert_eq!(
            tests_added_with_paths(p),
            vec![("src/lib.rs".to_string(), "added_case".to_string())]
        );
        assert_eq!(
            defs_in_line_for("A.java", "    public static int total(List<Item> items) {")
                .unwrap()
                .0,
            "total"
        );
        assert_eq!(
            defs_in_line_for("a.ts", "export const total = (items: Item[]): number => {")
                .unwrap()
                .0,
            "total"
        );
        assert_eq!(
            defs_in_line_for("a.ts", "  async total(items: Item[]): Promise<number> {")
                .unwrap()
                .0,
            "total"
        );
        assert!(defs_in_line_for("a.ts", "  if (x) {").is_none());
        assert_eq!(
            defs_in_line_for("a.kt", "fun total(items: List<Item>): Int {")
                .unwrap()
                .0,
            "total"
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
