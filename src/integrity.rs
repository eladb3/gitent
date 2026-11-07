//! Test integrity and orphan-copy checks (ADR 0013 §2, §6).
//!
//! Name-based guards cannot see a test that was kept but neutered: skipped,
//! stripped of assertions, or given new expected values. These checks compare
//! each test touched by a move with a *reference* version of the same test:
//! the moved change's version when the change itself touched that test, the
//! target's version otherwise.

use crate::diff;
use crate::model::CheckResult;
use crate::ops::StepKind;
use crate::preserve::file_text;
use crate::repo::Repo;
use anyhow::Result;
use git2::{Oid, Tree};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

fn skip_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(concat!(
            r"@(?:unittest\.)?skip(?:If|Unless)?\b|@(?:unittest\.)?expectedFailure\b|@pytest\.mark\.(?:skip|skipif|xfail)",
            r"|\.skipTest\(|\bpytest\.(?:skip|xfail|importorskip)\(|\braise\s+(?:unittest\.)?SkipTest\b",
            r"|#\[ignore|#\[should_panic|\b(?:it|test|describe)\.(?:skip|todo|failing)\b|\bx(?:it|test|describe)\s*\(",
            r"|\bt\.Skip(?:Now|f)?\(|@Disabled\b|@Ignore\b|\[Ignore\b|\[(?:Fact|Theory)\(\s*Skip\s*=|\bXCTSkip"
        ))
        .unwrap()
    })
}

fn assert_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(concat!(
            r"\bassert\w*!?|\bexpect\s*\(|\.should\b|\bt\.(?:Error|Errorf|Fatal|Fatalf|Fail)\b",
            r"|\brequire\.\w+|\bXCTAssert\w*|\bAssert\.\w+|\bcheck\s*\("
        ))
        .unwrap()
    })
}

fn literal_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|\b\d+(?:\.\d+)?\b|\b(?:True|False|true|false|None|null|nil)\b"#)
            .unwrap()
    })
}

/// Test name → block (attribute/decorator lines + header + body).
pub fn test_blocks(text: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = HashMap::new();
    for (name, i) in diff::tests_in_text(text) {
        let mut start = i;
        while start > 0 {
            let t = lines[start - 1].trim_start();
            if t.starts_with('@') || t.starts_with("#[") || (t.starts_with('[') && t.ends_with(']'))
            {
                start -= 1;
            } else {
                break;
            }
        }
        let base = indent(lines[i]);
        let mut end = i + 1;
        while end < lines.len() {
            let m = lines[end];
            if !m.trim().is_empty()
                && indent(m) <= base
                && !m.trim_start().starts_with('}')
                && !m.trim_start().starts_with(')')
            {
                break;
            }
            end += 1;
            if indent(m) == base && m.trim_start().starts_with('}') {
                break;
            }
        }
        out.insert(name, lines[start..end].join("\n").trim_end().to_string());
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub test: String,
    pub file: String,
    pub reason: String,
    /// true = only a human can judge it (unverified); false = definitive (rejected)
    pub review: bool,
}

fn assertions(block: &str) -> usize {
    block
        .lines()
        .map(|l| assert_re().find_iter(l).count())
        .sum()
}

fn assert_literals(block: &str) -> Vec<String> {
    let mut v: Vec<String> = block
        .lines()
        .filter(|l| assert_re().is_match(l))
        .flat_map(|l| {
            literal_re()
                .find_iter(l)
                .map(|m| m.as_str().to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    v.sort();
    v
}

fn multiset_diff(a: &[String], b: &[String]) -> Vec<String> {
    let mut m: BTreeMap<&str, i64> = BTreeMap::new();
    for x in a {
        *m.entry(x).or_default() += 1;
    }
    for x in b {
        *m.entry(x).or_default() -= 1;
    }
    m.into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(k, _)| k.to_string())
        .collect()
}

/// Compare a test's reference block with its block in the result.
pub fn compare(reference: &str, result: &str) -> Option<(String, bool)> {
    if reference == result {
        return None;
    }
    let skips = |b: &str| skip_re().find_iter(b).count();
    if skips(result) > skips(reference) {
        return Some(("a skip/ignore/xfail marker was added".into(), false));
    }
    let (ra, na) = (assertions(reference), assertions(result));
    if ra > 0 && na == 0 {
        return Some(("every assertion was removed".into(), true));
    }
    if na < ra {
        return Some((format!("fewer assertions ({ra} → {na})"), true));
    }
    let (rl, nl) = (assert_literals(reference), assert_literals(result));
    let removed = multiset_diff(&rl, &nl);
    if !removed.is_empty() {
        let added = multiset_diff(&nl, &rl);
        return Some((
            format!(
                "expected values changed on assertion lines (removed {}{})",
                removed.join(" "),
                if added.is_empty() {
                    String::new()
                } else {
                    format!("; added {}", added.join(" "))
                }
            ),
            true,
        ));
    }
    None
}

/// The moved change as (from, to) commits, and whose version of touched tests is the reference.
fn change_of(repo: &Repo, kind: &StepKind) -> Result<Option<(Oid, Oid, bool)>> {
    Ok(match kind {
        StepKind::Apply { source } | StepKind::Revert { source } => {
            let c = repo.git.find_commit(Oid::from_str(source)?)?;
            if c.parent_count() == 0 {
                return Ok(None);
            }
            // reference: the change's after-state (apply) or before-state (revert)
            Some((
                c.parent_id(0)?,
                c.id(),
                matches!(kind, StepKind::Apply { .. }),
            ))
        }
        StepKind::Merge { theirs, base, .. } => {
            Some((Oid::from_str(base)?, Oid::from_str(theirs)?, true))
        }
    })
}

/// `tests_intact`: tests the move added or modified, compared with their reference.
pub fn tests_intact(
    repo: &Repo,
    kind: &StepKind,
    base: Oid,
    result_tree: Oid,
) -> Result<CheckResult> {
    let base_t = repo.git.find_commit(base)?.tree()?;
    let res_t = repo.git.find_tree(result_tree)?;
    let moved = repo.diff_trees(Some(&base_t), &res_t)?;

    // Tests the moved change itself adds or modifies, and where.
    let mut touched: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut reference_tree: Option<Tree> = None;
    if let Some((from, to, after)) = change_of(repo, kind)? {
        let (ft, tt) = (
            repo.git.find_commit(from)?.tree()?,
            repo.git.find_commit(to)?.tree()?,
        );
        let cd = repo.diff_trees(Some(&ft), &tt)?;
        for f in &cd.files {
            let (a, b) = (file_text(repo, &ft, &f.path), file_text(repo, &tt, &f.path));
            let (ba, bb) = (
                a.as_deref().map(test_blocks).unwrap_or_default(),
                b.as_deref().map(test_blocks).unwrap_or_default(),
            );
            for (name, body) in &bb {
                if ba.get(name) != Some(body) {
                    touched
                        .entry(name.clone())
                        .or_default()
                        .insert(f.path.clone());
                }
            }
            for name in ba.keys() {
                if !bb.contains_key(name) {
                    touched
                        .entry(name.clone())
                        .or_default()
                        .insert(f.path.clone());
                }
            }
        }
        reference_tree = Some(if after { tt } else { ft });
    }
    let reference_block = |name: &str, path: &str| -> Option<String> {
        let rt = reference_tree.as_ref()?;
        let mut paths: Vec<String> = vec![path.to_string()];
        paths.extend(touched.get(name).into_iter().flatten().cloned());
        paths
            .iter()
            .find_map(|p| file_text(repo, rt, p).and_then(|t| test_blocks(&t).remove(name)))
    };

    let mut findings = vec![];
    let mut notes: Vec<String> = vec![];
    for f in &moved.files {
        if f.status == "deleted" {
            continue;
        }
        let Some(now) = file_text(repo, &res_t, &f.path) else {
            continue;
        };
        let before = file_text(repo, &base_t, f.old_path.as_deref().unwrap_or(&f.path))
            .map(|t| test_blocks(&t))
            .unwrap_or_default();
        for (name, block) in test_blocks(&now) {
            if before.get(&name) == Some(&block) {
                continue; // untouched by the move
            }
            let reference = if touched.contains_key(&name) {
                reference_block(&name, &f.path)
            } else {
                before.get(&name).cloned()
            };
            let Some(reference) = reference else { continue };
            if let Some((reason, review)) = compare(&reference, &block) {
                if review && touched.contains_key(&name) {
                    // The moved change's own tests are proved on the target
                    // (`ported_test_discriminates`, ADR 0015); heuristics are notes.
                    notes.push(format!("{}::{name}: {reason} (advisory)", f.path));
                    continue;
                }
                findings.push(Finding {
                    test: name,
                    file: f.path.clone(),
                    reason,
                    review,
                });
            }
        }
    }
    findings.sort_by(|a, b| (a.review, &a.file, &a.test).cmp(&(b.review, &b.file, &b.test)));
    Ok(CheckResult {
        name: "tests_intact".into(),
        passed: findings.is_empty(),
        review: !findings.is_empty() && findings.iter().all(|f| f.review),
        detail: if findings.is_empty() {
            if notes.is_empty() {
                "no test was skipped; no target test was weakened".into()
            } else {
                format!("no test was skipped; notes: {}", notes.join("; "))
            }
        } else {
            findings
                .iter()
                .map(|f| format!("{}::{}: {}", f.file, f.test, f.reason))
                .chain(notes.iter().cloned())
                .collect::<Vec<_>>()
                .join("; ")
        },
    })
}

/// Files that can change how tests run or what they exit with (ADR 0015 §5).
pub fn is_harness_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let in_test_dir = path
        .split('/')
        .rev()
        .skip(1)
        .any(|s| matches!(s, "test" | "tests" | "spec" | "__tests__" | "testing"));
    name == "conftest.py"
        || (name == "__init__.py" && in_test_dir)
        || matches!(
            name,
            "pytest.ini" | "tox.ini" | "setup.cfg" | "pyproject.toml" | "package.json"
        )
        || name.starts_with("jest.config")
        || name.starts_with("vitest.config")
        || name.starts_with(".mocharc")
        || name.starts_with("karma.conf")
        || path.starts_with(".cargo/config")
        || path == ".gitent/config.toml"
        || name.ends_with("_test.go")
}

/// Which changed lines of a harness file matter. Ordinary dependency bumps in
/// `package.json`/`pyproject.toml` and ordinary Go tests do not.
fn harness_relevant(path: &str, patch_lines: &[&str]) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name {
        "package.json" => patch_lines.iter().any(|l| {
            l.contains("\"test")
                || l.contains("\"jest\"")
                || l.contains("\"scripts\"")
                || l.contains("testFailureExitCode")
                || l.contains("\"mocha\"")
                || l.contains("\"vitest\"")
        }),
        "pyproject.toml" | "setup.cfg" | "tox.ini" => patch_lines.iter().any(|l| {
            l.contains("pytest")
                || l.contains("addopts")
                || l.contains("testpaths")
                || l.contains("unittest")
        }),
        _ if name.ends_with("_test.go") => patch_lines.iter().any(|l| l.contains("TestMain")),
        _ => true,
    }
}

/// `harness_unchanged` (review): the move changes a test-harness file the moved change did not.
pub fn harness_unchanged(
    repo: &Repo,
    kind: &StepKind,
    base: Oid,
    result_tree: Oid,
) -> Result<CheckResult> {
    let base_t = repo.git.find_commit(base)?.tree()?;
    let res_t = repo.git.find_tree(result_tree)?;
    let moved = repo.diff_trees(Some(&base_t), &res_t)?;
    let mut by_change: BTreeSet<String> = BTreeSet::new();
    if let Some((from, to, _)) = change_of(repo, kind)? {
        let (ft, tt) = (
            repo.git.find_commit(from)?.tree()?,
            repo.git.find_commit(to)?.tree()?,
        );
        by_change.extend(
            repo.diff_trees(Some(&ft), &tt)?
                .files
                .into_iter()
                .map(|f| f.path),
        );
    }
    // Changed lines per file, from the move's patch.
    let mut lines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cur = String::new();
    for l in moved.patch.lines() {
        if let Some(p) = l
            .strip_prefix("+++ b/")
            .or_else(|| l.strip_prefix("--- a/"))
        {
            cur = p.trim().to_string();
        } else if (l.starts_with('+') || l.starts_with('-'))
            && !l.starts_with("+++")
            && !l.starts_with("---")
        {
            lines.entry(cur.clone()).or_default().push(l.to_string());
        }
    }
    let mut changed: Vec<String> = moved
        .files
        .iter()
        .filter(|f| is_harness_file(&f.path) && !by_change.contains(&f.path))
        .filter(|f| {
            let ls: Vec<&str> = lines
                .get(&f.path)
                .map(|v| v.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default();
            harness_relevant(&f.path, &ls)
        })
        .map(|f| f.path.clone())
        .collect();
    changed.sort();
    Ok(CheckResult {
        name: "harness_unchanged".into(),
        passed: changed.is_empty(),
        review: !changed.is_empty(),
        detail: if changed.is_empty() {
            String::new()
        } else {
            format!(
                "changes test-harness file(s) that can alter how tests run or exit: {}",
                changed.join(", ")
            )
        },
    })
}

/// Definition name → block, for one code file (same block rules as tests).
fn def_blocks(path: &str, text: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        let Some((name, _)) = diff::defs_in_line_for(path, l) else {
            continue;
        };
        let base = indent(l);
        let mut end = i + 1;
        while end < lines.len() {
            let m = lines[end];
            if !m.trim().is_empty() && indent(m) <= base && !m.trim_start().starts_with('}') {
                break;
            }
            end += 1;
            if indent(m) == base && m.trim_start().starts_with('}') {
                break;
            }
        }
        // A body on the header line: `def f(x): return x * 2`, `fn f() -> u8 { 2 }`.
        let header = l.trim_end();
        let inline = if header.ends_with(':') || header.ends_with('{') {
            ""
        } else if let Some((_, rest)) = header.split_once("):") {
            rest
        } else if let Some((_, rest)) = header.split_once('{') {
            rest.trim_end_matches('}')
        } else {
            ""
        };
        let mut body = vec![inline.trim().to_string()];
        body.extend(lines[i + 1..end].iter().map(|x| x.to_string()));
        out.entry(name).or_insert_with(|| body.join("\n"));
    }
    out
}

/// Meaningful body lines: not blank, not a brace, not a placeholder statement.
fn substance(body: &str) -> usize {
    body.lines()
        .map(|l| l.trim())
        .filter(|l| {
            !l.is_empty()
                && !matches!(
                    *l,
                    "{" | "}"
                        | "};"
                        | "pass"
                        | "..."
                        | "return"
                        | "return;"
                        | "return None"
                        | "todo!()"
                        | "unimplemented!()"
                        | "throw new UnsupportedOperationException();"
                )
                && !l.starts_with('#')
                && !l.starts_with("//")
                && !l.starts_with("\"\"\"")
        })
        .count()
}

/// `bodies_intact` (review): a definition the target had with a real body, which the move
/// reduces to a stub (and the moved change itself did not). Name-based guards cannot see it.
pub fn bodies_intact(
    repo: &Repo,
    kind: &StepKind,
    base: Oid,
    result_tree: Oid,
) -> Result<CheckResult> {
    let base_t = repo.git.find_commit(base)?.tree()?;
    let res_t = repo.git.find_tree(result_tree)?;
    let moved = repo.diff_trees(Some(&base_t), &res_t)?;
    // Definitions the moved change itself stubs out may legitimately be stubbed.
    let mut stubbed_by_change: BTreeSet<String> = BTreeSet::new();
    if let Some((from, to, _)) = change_of(repo, kind)? {
        let (ft, tt) = (
            repo.git.find_commit(from)?.tree()?,
            repo.git.find_commit(to)?.tree()?,
        );
        for f in &repo.diff_trees(Some(&ft), &tt)?.files {
            let a = file_text(repo, &ft, &f.path)
                .map(|t| def_blocks(&f.path, &t))
                .unwrap_or_default();
            let b = file_text(repo, &tt, &f.path)
                .map(|t| def_blocks(&f.path, &t))
                .unwrap_or_default();
            for (n, body) in &b {
                if a.get(n)
                    .map(|o| substance(o) > substance(body))
                    .unwrap_or(false)
                {
                    stubbed_by_change.insert(n.clone());
                }
            }
        }
    }
    let mut stubbed = vec![];
    for f in moved.files.iter().filter(|f| f.status == "modified") {
        if !crate::preserve::is_code(&f.path) || crate::requirements::is_test_file(&f.path) {
            continue;
        }
        let (Some(a), Some(b)) = (
            file_text(repo, &base_t, &f.path),
            file_text(repo, &res_t, &f.path),
        ) else {
            continue;
        };
        let (old, new) = (def_blocks(&f.path, &a), def_blocks(&f.path, &b));
        for (n, body) in &new {
            if let Some(o) = old.get(n) {
                if substance(o) >= 1 && substance(body) == 0 && !stubbed_by_change.contains(n) {
                    stubbed.push(format!("{}::{n}", f.path));
                }
            }
        }
    }
    stubbed.sort();
    Ok(CheckResult {
        name: "bodies_intact".into(),
        passed: stubbed.is_empty(),
        review: !stubbed.is_empty(),
        detail: if stubbed.is_empty() {
            String::new()
        } else {
            format!(
                "definitions of the target reduced to stubs: {}",
                stubbed.join(", ")
            )
        },
    })
}

/// `no_orphan_copy` (review): the move creates a file the source only modified and the
/// target never had, i.e. it copied the source branch's code instead of adapting the target's.
pub fn no_orphan_copy(
    repo: &Repo,
    kind: &StepKind,
    base: Oid,
    result_tree: Oid,
) -> Result<Option<CheckResult>> {
    let StepKind::Apply { source } = kind else {
        return Ok(None);
    };
    let src = repo.git.find_commit(Oid::from_str(source)?)?;
    if src.parent_count() == 0 {
        return Ok(None);
    }
    let sd = repo.diff_trees(Some(&src.parent(0)?.tree()?), &src.tree()?)?;
    let base_t = repo.git.find_commit(base)?.tree()?;
    let res_t = repo.git.find_tree(result_tree)?;
    let orphans: Vec<String> = sd
        .files
        .iter()
        .filter(|f| {
            f.status == "modified"
                && crate::preserve::is_code(&f.path)
                && !crate::requirements::is_test_file(&f.path)
        })
        .filter(|f| base_t.get_path(std::path::Path::new(&f.path)).is_err())
        .filter(|f| res_t.get_path(std::path::Path::new(&f.path)).is_ok())
        .map(|f| f.path.clone())
        .collect();
    // Also: the source fixed existing code, but the port changes none of the target's
    // existing code files, only adds new ones (e.g. a copy as `cart_v2.py`).
    let mut orphans = orphans;
    let src_fixes_code = sd.files.iter().any(|f| {
        f.status == "modified"
            && crate::preserve::is_code(&f.path)
            && !crate::requirements::is_test_file(&f.path)
    });
    let moved = repo.diff_trees(Some(&base_t), &res_t)?;
    let code = |f: &&crate::repo::FileChange| {
        crate::preserve::is_code(&f.path) && !crate::requirements::is_test_file(&f.path)
    };
    let touches_existing = moved.files.iter().filter(code).any(|f| f.status != "added");
    let adds: Vec<String> = moved
        .files
        .iter()
        .filter(code)
        .filter(|f| f.status == "added")
        .map(|f| f.path.clone())
        .collect();
    if orphans.is_empty() && src_fixes_code && !touches_existing && !adds.is_empty() {
        orphans = adds;
    }
    Ok(Some(CheckResult {
        name: "no_orphan_copy".into(),
        passed: orphans.is_empty(),
        review: !orphans.is_empty(),
        detail: if orphans.is_empty() {
            String::new()
        } else {
            format!(
                "creates {} while the source change modified existing code: the port may copy \
                 the source branch's code instead of fixing the target's",
                orphans.join(", ")
            )
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REF: &str = "class T(unittest.TestCase):\n    def test_clamp(self):\n        self.assertEqual(total(150), 0)\n        self.assertEqual(total(-5), 100)\n";

    fn block(t: &str) -> String {
        test_blocks(t).remove("test_clamp").unwrap()
    }

    #[test]
    fn detects_skip_weakening_and_new_expectations() {
        let r = block(REF);
        let skipped = REF.replace(
            "    def test_clamp",
            "    @unittest.skip(\"later\")\n    def test_clamp",
        );
        assert!(
            !compare(&r, &block(&skipped)).unwrap().1,
            "skip is definitive"
        );
        let weaker = REF.replace("        self.assertEqual(total(-5), 100)\n", "");
        assert!(
            compare(&r, &block(&weaker)).unwrap().1,
            "fewer assertions: review"
        );
        let emptied =
            "class T(unittest.TestCase):\n    def test_clamp(self):\n        total(150)\n";
        assert!(
            compare(&r, &block(emptied)).unwrap().1,
            "advisory: the proof on the target decides"
        );
        let rewritten = REF.replace("total(150), 0)", "total(150), 150)");
        let (why, review) = compare(&r, &block(&rewritten)).unwrap();
        assert!(review && why.contains("0"), "{why}");
        // Renaming the API under test is fine.
        let renamed = REF.replace("total(", "cart_total(");
        assert!(compare(&r, &block(&renamed)).is_none());
    }

    #[test]
    fn skip_markers_are_precise() {
        let r = "    def test_rows(self):\n        self.assertEqual(f(), 1)\n";
        let n =
            "    def test_rows(self):\n        rowsToSkip = 3\n        self.assertEqual(f(), 1)\n";
        assert!(
            compare(&block_of(r), &block_of(n))
                .map(|x| x.1)
                .unwrap_or(true),
            "not a skip"
        );
        for marker in [
            "@skip(\"x\")",
            "@unittest.expectedFailure",
            "@pytest.mark.skipif(True, reason=\"\")",
        ] {
            let m = format!("    {marker}\n{r}");
            assert_eq!(
                compare(&block_of(r), &block_of(&m)).map(|x| x.1),
                Some(false),
                "{marker}"
            );
        }
        let raise = r.replace(
            "        self.assertEqual",
            "        raise unittest.SkipTest(\"x\")\n        self.assertEqual",
        );
        assert_eq!(
            compare(&block_of(r), &block_of(&raise)).map(|x| x.1),
            Some(false)
        );
    }

    fn block_of(t: &str) -> String {
        test_blocks(t).remove("test_rows").unwrap()
    }

    #[test]
    fn one_line_bodies_are_not_stubs() {
        let b = def_blocks("a.py", "def double(x): return x * 2\n");
        assert_eq!(substance(&b["double"]), 1, "{b:?}");
        let b = def_blocks("a.rs", "fn two() -> u8 { 2 }\n");
        assert_eq!(substance(&b["two"]), 1, "{b:?}");
    }

    #[test]
    fn stub_detection() {
        let py = "def total(p, d):\n    s = sum(p)\n    return s - s * d / 100\n\ndef other():\n    pass\n";
        let b = def_blocks("a.py", py);
        assert_eq!(substance(&b["total"]), 2);
        assert_eq!(substance(&b["other"]), 0);
        let rs = "fn total(p: &[u32]) -> u32 {\n    let s: u32 = p.iter().sum();\n    s\n}\nfn stub() {\n    todo!()\n}\n";
        let b = def_blocks("a.rs", rs);
        assert_eq!(substance(&b["total"]), 2, "{b:?}");
        assert_eq!(substance(&b["stub"]), 0, "{b:?}");
    }

    #[test]
    fn blocks_include_attributes_and_braces() {
        let rs = "#[test]\n#[ignore]\nfn test_clamp() {\n    assert_eq!(total(150), 0);\n}\n\nfn helper() {}\n";
        let b = test_blocks(rs).remove("test_clamp").unwrap();
        assert!(
            b.starts_with("#[test]") && b.ends_with('}') && !b.contains("helper"),
            "{b}"
        );
        let js = "describe('cart', () => {\n  it('clamps', () => {\n    expect(total(150)).toBe(0);\n  });\n  it('other', () => {});\n});\n";
        let b = test_blocks(js).remove("clamps").unwrap();
        assert!(b.contains("toBe(0)") && !b.contains("other"), "{b}");
    }
}
