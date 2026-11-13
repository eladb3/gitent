//! `gitent prove-pr`: prove that a pull request's tests test its change (ADR 0017).
//!
//! Every test the PR adds or modifies must FAIL on the base with the PR's test
//! files overlaid (the code unchanged) and PASS on the PR. A test that passes
//! without the change does not test it, however it is written.

use super::Session;
use crate::integrity;
use crate::model::{ReqKind, Requirement};
use crate::preserve::{file_text, is_code};
use crate::repo::short;
use crate::requirements::{is_test_file, overlay_test_files, Runner, TempTree};
use anyhow::{bail, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub const MARKER: &str = "<!-- gitent-prove-pr -->";
pub const SCHEMA: &str = "gitent/prove-pr@1";

#[derive(Debug, Clone, Default)]
pub struct ProveOptions {
    pub base: String,
    pub head: Option<String>,
    pub working_tree: bool,
    /// Every added/modified test must prove the change (default: at least one).
    pub require_all: bool,
}

/// Why a test failed: `error` = something it uses does not exist / does not build
/// (it proves the API exists, not the behaviour); `assertion` = a real check failed.
pub fn failure_kind(output: &str) -> &'static str {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(concat!(
            r"ModuleNotFoundError|ImportError|cannot import name|NameError|has no attribute",
            r"|SyntaxError|IndentationError|is not defined|is not a function|Cannot find module",
            r"|error\[E\d+\]|cannot find (?:function|value|type|struct|module)|unresolved import",
            r"|undefined: |no field or method|could not compile|compilation failed",
            r"|cannot find symbol|does not exist in the current context|Unresolved reference"
        ))
        .unwrap()
    });
    if re.is_match(output) {
        "error"
    } else {
        "assertion"
    }
}

pub struct PrTest {
    pub name: String,
    pub file: String,
    pub change: &'static str, // added | modified
}

/// What a change (`onto..head`) does to code and tests, without running anything.
pub struct Shape {
    pub code_files: Vec<String>,
    pub tests: Vec<PrTest>,
    pub skipped: Vec<String>,
    pub deleted: Vec<String>,
    pub notes: Vec<String>,
}

/// Classify a change: code files, tests added/modified, existing tests skipped/deleted.
pub fn shape(repo: &crate::repo::Repo, onto: git2::Oid, head: git2::Oid) -> Result<Shape> {
    let (ot, ht) = (
        repo.git.find_commit(onto)?.tree()?,
        repo.git.find_commit(head)?.tree()?,
    );
    let d = repo.diff_trees(Some(&ot), &ht)?;

    let code_files: Vec<String> = d
        .files
        .iter()
        .filter(|f| is_code(&f.path) && !is_test_file(&f.path))
        .map(|f| f.path.clone())
        .collect();

    // Tests the PR adds or modifies; existing tests it skips or deletes.
    let mut tests: Vec<PrTest> = vec![];
    let mut skipped: Vec<String> = vec![];
    let mut notes: Vec<String> = vec![];
    let mut old_names: BTreeMap<String, String> = BTreeMap::new(); // name -> file, at onto
    let mut new_names: BTreeSet<String> = BTreeSet::new();
    for f in &d.files {
        let old_path = f.old_path.as_deref().unwrap_or(&f.path);
        let old = file_text(repo, &ot, old_path)
            .map(|t| integrity::test_blocks(&t))
            .unwrap_or_default();
        let new = if f.status == "deleted" {
            Default::default()
        } else {
            file_text(repo, &ht, &f.path)
                .map(|t| integrity::test_blocks(&t))
                .unwrap_or_default()
        };
        for n in old.keys() {
            old_names.insert(n.clone(), old_path.to_string());
        }
        for (name, block) in &new {
            new_names.insert(name.clone());
            match old.get(name) {
                Some(prev) if prev == block => {}
                Some(prev) => {
                    if let Some((reason, review)) = integrity::compare(prev, block) {
                        if !review {
                            skipped.push(format!("{}::{name}: {reason}", f.path));
                            continue;
                        }
                        notes.push(format!("{}::{name}: {reason}", f.path));
                    }
                    tests.push(PrTest {
                        name: name.clone(),
                        file: f.path.clone(),
                        change: "modified",
                    });
                }
                None => tests.push(PrTest {
                    name: name.clone(),
                    file: f.path.clone(),
                    change: "added",
                }),
            }
        }
    }
    // A test "deleted" here may have moved to a file the PR did not otherwise touch.
    let head_index = crate::preserve::test_index(repo, &ht);
    let deleted: Vec<String> = old_names
        .iter()
        .filter(|(n, _)| !new_names.contains(*n) && !head_index.contains_key(*n))
        .map(|(n, f)| format!("{f}::{n}"))
        .collect();

    Ok(Shape {
        code_files,
        tests,
        skipped,
        deleted,
        notes,
    })
}

/// Run each test with the change (`head`) and without it (`onto` + head's test files).
/// One JSON row per test: status proven | proven_api | passes_without_change | fails_on_pr | not_run.
pub fn prove_tests(
    s: &Session,
    onto: git2::Oid,
    head: git2::Oid,
    tests: &[PrTest],
) -> Result<Vec<Value>> {
    let repo = &s.repo;
    // The proof: with the change (head) and without it (onto + the PR's test files).
    let reqs: Vec<Requirement> = tests
        .iter()
        .enumerate()
        .map(|(i, t)| Requirement {
            id: format!("t{}", i + 1),
            statement: t.name.clone(),
            kind: ReqKind::Test,
            test: t.name.clone(),
            test_path: t.file.clone(),
            origin: "auto".into(),
            ..Default::default()
        })
        .collect();
    let mut with = vec![];
    let mut without = vec![];
    if !reqs.is_empty() {
        let t = TempTree::new(repo, head, "prove-with")?;
        let mut run = Runner::new(&t.path, &s.cfg);
        with = reqs.iter().map(|r| run.run(r)).collect();
        drop(t);
        let t = TempTree::new(repo, onto, "prove-without")?;
        overlay_test_files(repo, onto, head, &t.path)?;
        let mut run = Runner::new(&t.path, &s.cfg);
        without = reqs.iter().map(|r| run.run(r)).collect();
    }
    let mut rows = vec![];
    for (i, t) in tests.iter().enumerate() {
        let (w_st, w_detail, _) = &with[i];
        let (wo_st, wo_detail, _) = &without[i];
        let (status, detail) = if w_st == "fail" {
            ("fails_on_pr", crate::proc::tail(w_detail, 1500))
        } else if w_st != "pass" || wo_st == "skipped" {
            ("not_run", w_detail.clone())
        } else if wo_st == "pass" {
            ("passes_without_change", String::new())
        } else if failure_kind(wo_detail) == "error" {
            ("proven_api", crate::proc::tail(wo_detail, 600))
        } else {
            ("proven", String::new())
        };
        rows.push(json!({
            "test": t.name, "file": t.file, "change": t.change,
            "without_change": wo_st, "with_change": w_st,
            "status": status, "detail": detail,
        }));
    }
    Ok(rows)
}

pub fn prove_pr(s: &Session, o: &ProveOptions) -> Result<Value> {
    let repo = &s.repo;
    let base_tip = repo.resolve(&o.base)?;
    // The PR's result: a commit, or a snapshot of the working tree.
    let head = if o.working_tree {
        if o.head.is_some() {
            bail!("--working-tree and --head are mutually exclusive");
        }
        let h = repo
            .head_commit()?
            .ok_or_else(|| anyhow::anyhow!("no HEAD to compare the working tree with"))?;
        let mut idx = repo.git.index()?;
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        idx.update_all(["*"].iter(), None)?;
        let tree = idx.write_tree()?;
        idx.read(true)?; // the user's index is untouched
        repo.commit(
            None,
            "gitent prove-pr: working tree snapshot",
            tree,
            &[h.id()],
        )?
    } else {
        repo.resolve(o.head.as_deref().unwrap_or("HEAD"))?
    };
    let onto = repo.merge_base(base_tip, head)?;
    let Shape {
        code_files,
        tests,
        skipped,
        deleted,
        notes,
    } = shape(repo, onto, head)?;
    let rows = prove_tests(s, onto, head, &tests)?;
    let count = |st: &str| rows.iter().filter(|r| r["status"] == st).count();
    let proving = count("proven") + count("proven_api");
    let (result, reason) = if !skipped.is_empty() {
        ("rejected", "the PR skips existing tests".to_string())
    } else if count("fails_on_pr") > 0 {
        ("rejected", "tests fail on the PR".to_string())
    } else if code_files.is_empty() {
        (
            "neutral",
            "no code changed (docs or tests only)".to_string(),
        )
    } else if !deleted.is_empty() {
        (
            "unproven",
            format!("the PR deletes {} existing test(s)", deleted.len()),
        )
    } else if tests.is_empty() {
        (
            "unproven",
            "code changed but no test was added or modified".to_string(),
        )
    } else if proving == 0 && count("not_run") == tests.len() {
        (
            "unproven",
            "the tests could not be run (no test_selector / test_command)".to_string(),
        )
    } else if proving == 0 {
        (
            "unproven",
            "no test fails without the change: none of them tests it".to_string(),
        )
    } else if o.require_all && count("passes_without_change") > 0 {
        (
            "unproven",
            format!(
                "--require-all: {} test(s) pass without the change",
                count("passes_without_change")
            ),
        )
    } else {
        (
            "proven",
            format!(
                "{proving} of {} test(s) fail without the change and pass with it",
                tests.len()
            ),
        )
    };
    let mut v = json!({
        "schema": SCHEMA,
        "result": result,
        "state": if matches!(result, "proven" | "neutral") { "succeeded" } else { "failed" },
        "reason": reason,
        "base": base_tip.to_string(), "onto": onto.to_string(),
        "head": if o.working_tree { "working-tree".to_string() } else { head.to_string() },
        "code_files": code_files,
        "tests": rows,
        "skipped_existing": skipped,
        "deleted_tests": deleted,
        "notes": notes,
    });
    if !matches!(result, "proven" | "neutral") {
        v["error"] = json!(format!("{result}: {reason}"));
    }
    v["markdown"] = json!(markdown(&v));
    Ok(v)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The PR comment / step summary.
pub fn markdown(v: &Value) -> String {
    use super::verify_pr::{code_inline, fence_for, md_inline};
    let mut m = format!("{MARKER}\n");
    let (icon, title) = match s(v, "result") {
        "proven" => ("✅", "tests prove the change"),
        "unproven" => ("⚠️", "tests do not prove the change"),
        "rejected" => ("❌", "tests rejected"),
        _ => ("➖", "nothing to prove"),
    };
    m.push_str(&format!(
        "### {icon} Gitent: {title}\n\n{}.\n\n",
        md_inline(s(v, "reason"))
    ));
    let tests = v["tests"].as_array().cloned().unwrap_or_default();
    if !tests.is_empty() {
        m.push_str("| test | without the change | with the change | |\n|---|---|---|---|\n");
        let mut details = vec![];
        for t in &tests {
            let (mark, what) = match s(t, "status") {
                "proven" => ("✅", "proves the change"),
                "proven_api" => (
                    "☑️",
                    "fails before only because the code it calls did not exist",
                ),
                "passes_without_change" => ("⚠️", "passes without the change: does not test it"),
                "fails_on_pr" => ("❌", "fails on the PR"),
                _ => ("➖", "not run"),
            };
            m.push_str(&format!(
                "| `{}` <sub>{}</sub> | {} | {} | {mark} {what} |\n",
                code_inline(s(t, "test")),
                md_inline(s(t, "file")),
                s(t, "without_change"),
                s(t, "with_change"),
            ));
            let d = s(t, "detail");
            if !d.is_empty() && s(t, "status") == "fails_on_pr" {
                details.push((s(t, "test").to_string(), d.to_string()));
            }
        }
        for (name, d) in details {
            let fence = fence_for(&d);
            m.push_str(&format!(
                "\n<details><summary>{}: output</summary>\n\n{fence}\n{d}\n{fence}\n</details>\n",
                md_inline(&name)
            ));
        }
    }
    for (key, label) in [
        ("skipped_existing", "❌ existing test skipped"),
        ("deleted_tests", "⚠️ existing test deleted"),
        ("notes", "note"),
    ] {
        for x in v[key].as_array().into_iter().flatten() {
            m.push_str(&format!(
                "\n- {label}: {}",
                md_inline(x.as_str().unwrap_or(""))
            ));
        }
    }
    m.push_str(&format!(
        "\n\n<sub>Checked by [gitent prove-pr](https://github.com/eladb3/gitent/blob/main/docs/adr/0017-prove-pr.md): each new or changed test is run on the base with the PR's tests but without its code (it must fail) and on the PR (it must pass). Base `{}`, head `{}`.</sub>\n",
        short(s(v, "onto")),
        short(s(v, "head"))
    ));
    m
}

#[cfg(test)]
mod tests {
    use super::failure_kind;

    #[test]
    fn classifies_failures() {
        assert_eq!(
            failure_kind("ImportError: cannot import name 'clamp' from 'shop'"),
            "error"
        );
        assert_eq!(
            failure_kind("AttributeError: module 'shop.calc' has no attribute 'clamp'"),
            "error"
        );
        assert_eq!(
            failure_kind("error[E0425]: cannot find function `clamp` in this scope"),
            "error"
        );
        assert_eq!(failure_kind("./a_test.go:9: undefined: Clamp"), "error");
        assert_eq!(failure_kind("AssertionError: 150 != 0"), "assertion");
        assert_eq!(
            failure_kind("expect(received).toBe(expected)\nExpected: 0\nReceived: 150"),
            "assertion"
        );
    }
}
