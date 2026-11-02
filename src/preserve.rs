//! Target-preservation guards (ADR 0010): a result must not resurrect paths the
//! target deliberately deleted, nor drop definitions the target has, except
//! where the change being moved itself does so.
//!
//! Found by fault injection (`evals/run.py --agent stale`): restoring the
//! original change's files verbatim passes the original tests against
//! resurrected old modules, so test-based checks alone cannot see it.

use crate::diff;
use crate::model::CheckResult;
use crate::ops::StepKind;
use crate::repo::Repo;
use anyhow::Result;
use git2::{Oid, Tree};
use std::collections::BTreeSet;

fn walk(repo: &Repo, tree: &Tree, mut f: impl FnMut(&str, &[u8])) {
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            let path = format!("{root}{}", entry.name().unwrap_or(""));
            if let Ok(b) = repo.git.find_blob(entry.id()) {
                f(&path, b.content());
            }
        }
        git2::TreeWalkResult::Ok
    });
}

pub fn paths(repo: &Repo, tree: &Tree) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(repo, tree, |p, _| {
        out.insert(p.to_string());
    });
    out
}

const CODE_EXT: &[&str] = &[
    "py", "rs", "js", "jsx", "ts", "tsx", "mjs", "cjs", "go", "rb", "java", "kt", "scala", "swift",
    "c", "h", "cc", "cpp", "hpp", "cs", "php",
];

/// Only source files can define things; prose ("a class does not…") must not count.
pub fn is_code(path: &str) -> bool {
    path.rsplit_once('.')
        .map(|(_, ext)| CODE_EXT.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Names of non-test definitions (functions, classes, types) in a tree's source files.
pub fn defs(repo: &Repo, tree: &Tree) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    walk(repo, tree, |path, content| {
        if !is_code(path) {
            return;
        }
        if content.len() > 1_000_000 || content.contains(&0) {
            return;
        }
        for line in String::from_utf8_lossy(content).lines() {
            if let Some((name, _)) = diff::defs_in_line_for(path, line) {
                if !name.starts_with("test") && !name.starts_with("Test") {
                    out.insert(name);
                }
            }
        }
    });
    out
}

fn tree_of(repo: &Repo, commit: Oid) -> Result<Tree<'_>> {
    Ok(repo.git.find_commit(commit)?.tree()?)
}

fn diff_set(a: &BTreeSet<String>, b: &BTreeSet<String>) -> BTreeSet<String> {
    a.difference(b).cloned().collect()
}

/// Run both guards for one step. `tip` is the step's starting commit.
pub fn check(repo: &Repo, kind: &StepKind, tip: Oid, result: Oid) -> Result<CheckResult> {
    let tip_t = tree_of(repo, tip)?;
    let res_t = repo.git.find_tree(result)?;
    let (p_tip, p_res) = (paths(repo, &tip_t), paths(repo, &res_t));
    let (d_tip, d_res) = (defs(repo, &tip_t), defs(repo, &res_t));

    // (paths the target deleted, paths the change itself legitimately re-adds,
    //  definitions the change itself legitimately removes, extra required defs)
    let (deleted_by_target, readded_ok, removed_ok, extra_required) = match kind {
        StepKind::Apply { source } => {
            let src = repo.git.find_commit(Oid::from_str(source)?)?;
            if src.parent_count() == 0 {
                return Ok(ok("no parent to compare"));
            }
            let parent = src.parent(0)?;
            let base = repo.merge_base(src.id(), tip).unwrap_or(parent.id());
            let (src_t, par_t, base_t) = (src.tree()?, parent.tree()?, tree_of(repo, base)?);
            (
                diff_set(&paths(repo, &base_t), &p_tip),
                diff_set(&paths(repo, &src_t), &paths(repo, &par_t)),
                diff_set(&defs(repo, &par_t), &defs(repo, &src_t)),
                BTreeSet::new(),
            )
        }
        StepKind::Revert { source } => {
            let src = repo.git.find_commit(Oid::from_str(source)?)?;
            if src.parent_count() == 0 {
                return Ok(ok("no parent to compare"));
            }
            let (src_t, par_t) = (src.tree()?, src.parent(0)?.tree()?);
            let (p_src, p_par) = (paths(repo, &src_t), paths(repo, &par_t));
            // Paths that existed before the reverted change but the target no longer has
            // (deleted later), excluding paths the reverted change itself deleted.
            (
                diff_set(&p_par, &p_tip),
                diff_set(&p_par, &p_src),
                diff_set(&defs(repo, &src_t), &defs(repo, &par_t)),
                BTreeSet::new(),
            )
        }
        StepKind::Merge { theirs, base, .. } => {
            let (th_t, base_t) = (
                tree_of(repo, Oid::from_str(theirs)?)?,
                tree_of(repo, Oid::from_str(base)?)?,
            );
            let (d_th, d_base) = (defs(repo, &th_t), defs(repo, &base_t));
            // Their definitions must survive too, unless our side removed them.
            let theirs_required = diff_set(&d_th, &diff_set(&d_base, &d_tip));
            (
                diff_set(&paths(repo, &base_t), &p_tip),
                diff_set(&paths(repo, &th_t), &paths(repo, &base_t)),
                diff_set(&d_base, &d_th),
                theirs_required,
            )
        }
    };

    let resurrected: Vec<String> = p_res
        .difference(&p_tip)
        .filter(|p| deleted_by_target.contains(*p) && !readded_ok.contains(*p))
        .cloned()
        .collect();
    let required: BTreeSet<String> = diff_set(&d_tip, &removed_ok)
        .union(&extra_required)
        .cloned()
        .collect();
    let missing: Vec<String> = required.difference(&d_res).cloned().collect();

    let mut problems = vec![];
    if !resurrected.is_empty() {
        problems.push(format!(
            "recreates path(s) the target deliberately deleted: {} (port the change to the target's current structure instead)",
            resurrected.join(", ")
        ));
    }
    if !missing.is_empty() {
        problems.push(format!(
            "removes definition(s) the target still has: {} (the change being moved does not remove them)",
            missing.join(", ")
        ));
    }
    Ok(CheckResult {
        name: "target_preserved".into(),
        passed: problems.is_empty(),
        detail: if problems.is_empty() {
            format!(
                "{} definition(s) of the target preserved; no deleted paths recreated",
                required.len()
            )
        } else {
            problems.join("; ")
        },
    })
}

fn ok(detail: &str) -> CheckResult {
    CheckResult {
        name: "target_preserved".into(),
        passed: true,
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_code_files_define_things() {
        assert!(super::is_code("django/db/models/query.py"));
        assert!(super::is_code("src/App.TSX"));
        assert!(!super::is_code("docs/ref/models.txt"));
        assert!(!super::is_code("README.md"));
        assert!(!super::is_code("Makefile"));
    }
}
