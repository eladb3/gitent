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
            r"@unittest\.skip|@pytest\.mark\.(?:skip|xfail)|\.skipTest\(|\bpytest\.(?:skip|xfail)\(",
            r"|#\[ignore|\b(?:it|test|describe)\.(?:skip|todo)\b|\bx(?:it|test|describe)\s*\(",
            r"|\bt\.Skip(?:Now|f)?\(|@Disabled\b|@Ignore\b|\[Ignore\b|Skip\s*=|\bXCTSkip"
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
        return Some(("every assertion was removed".into(), false));
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
            "no test was skipped, stripped of assertions or given new expected values".into()
        } else {
            findings
                .iter()
                .map(|f| format!("{}::{}: {}", f.file, f.test, f.reason))
                .collect::<Vec<_>>()
                .join("; ")
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
    Ok(Some(CheckResult {
        name: "no_orphan_copy".into(),
        passed: orphans.is_empty(),
        review: !orphans.is_empty(),
        detail: if orphans.is_empty() {
            String::new()
        } else {
            format!(
                "creates {} which the source change only modified and the target does not have: \
                 the port may copy the source branch's code instead of fixing the target's",
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
        assert!(!compare(&r, &block(&skipped)).unwrap().1, "skip is definitive");
        let weaker = REF.replace("        self.assertEqual(total(-5), 100)\n", "");
        assert!(
            compare(&r, &block(&weaker)).unwrap().1,
            "fewer assertions: review"
        );
        let emptied =
            "class T(unittest.TestCase):\n    def test_clamp(self):\n        total(150)\n";
        assert!(!compare(&r, &block(emptied)).unwrap().1);
        let rewritten = REF.replace("total(150), 0)", "total(150), 150)");
        let (why, review) = compare(&r, &block(&rewritten)).unwrap();
        assert!(review && why.contains("0"), "{why}");
        // Renaming the API under test is fine.
        let renamed = REF.replace("total(", "cart_total(");
        assert!(compare(&r, &block(&renamed)).is_none());
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
