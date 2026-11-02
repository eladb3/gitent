//! Semantic commit and semantic diff.

use super::Session;
use crate::analyze;
use crate::model::{IntentDraft, SemanticChange};
use crate::store;
use anyhow::{bail, Result};

#[derive(Debug, Default, Clone)]
pub struct CommitOptions {
    pub message: Option<String>,
    pub intent: Option<IntentDraft>,
    /// Commit only what is staged (default: stage everything like `git add -A`).
    pub staged_only: bool,
    pub require_llm: bool,
    pub dry_run: bool,
}

pub struct CommitResult {
    pub commit: Option<String>,
    pub record: SemanticChange,
}

pub fn commit(s: &Session, o: &CommitOptions) -> Result<CommitResult> {
    let repo = &s.repo;
    let head = repo.head_commit()?;
    let head_tree = match &head {
        Some(c) => Some(c.tree()?),
        None => None,
    };
    let tree_oid = if o.staged_only {
        repo.git.index()?.write_tree()?
    } else if o.dry_run {
        // Do not touch the real index on a dry run.
        let mut idx = repo.git.index()?;
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        idx.update_all(["*"].iter(), None)?;
        idx.write_tree()?
    } else {
        repo.stage_all()?
    };
    let tree = repo.git.find_tree(tree_oid)?;
    if head_tree.as_ref().map(|t| t.id()) == Some(tree_oid) {
        bail!("nothing to commit (working tree matches HEAD)");
    }
    let d = repo.diff_trees(head_tree.as_ref(), &tree)?;
    let message = o.message.clone().unwrap_or_default();
    let a = analyze::analyze(s.provider(), o.intent.clone(), &message, &d, o.require_llm)?;
    let mut rec = analyze::build_record(a, &d, "commit");
    if o.dry_run {
        if !o.staged_only {
            // Restore the on-disk index we modified in memory.
            repo.git.index()?.read(true)?;
        }
        return Ok(CommitResult {
            commit: None,
            record: rec,
        });
    }
    let msg = analyze::commit_message(&rec, o.message.as_deref());
    let parents: Vec<git2::Oid> = head.iter().map(|c| c.id()).collect();
    let oid = repo.commit(Some("HEAD"), &msg, tree_oid, &parents)?;
    rec.realization = store::realization(repo, oid)?;
    store::put(repo, oid, &rec)?;
    Ok(CommitResult {
        commit: Some(oid.to_string()),
        record: rec,
    })
}

/// Analyze uncommitted changes without committing.
pub fn semantic_diff(s: &Session) -> Result<(crate::repo::DiffInfo, SemanticChange)> {
    let d = s.repo.diff_workdir()?;
    if d.files.is_empty() {
        bail!("no changes");
    }
    let a = analyze::analyze(s.provider(), None, "", &d, false)?;
    let rec = analyze::build_record(a, &d, "commit");
    Ok((d, rec))
}
