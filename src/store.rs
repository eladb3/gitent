//! Semantic records in `refs/notes/gitent` (ADR 0002).

use crate::analyze;
use crate::llm::Provider;
use crate::model::{Realization, SemanticChange};
use crate::repo::Repo;
use anyhow::{Context, Result};
use git2::Oid;

pub fn get(repo: &Repo, oid: Oid) -> Option<SemanticChange> {
    let body = repo.read_note(oid)?;
    serde_json::from_str(&body).ok()
}

pub fn put(repo: &Repo, oid: Oid, rec: &SemanticChange) -> Result<()> {
    repo.write_note(oid, &serde_json::to_string_pretty(rec)?)
}

pub fn realization(repo: &Repo, oid: Oid) -> Result<Realization> {
    let c = repo.git.find_commit(oid)?;
    Ok(Realization {
        commit: oid.to_string(),
        parents: c.parent_ids().map(|p| p.to_string()).collect(),
        tree: c.tree_id().to_string(),
    })
}

/// All realisations of an intention.
pub fn find_by_intent(repo: &Repo, intent_id: &str) -> Result<Vec<(Oid, SemanticChange)>> {
    let mut out = vec![];
    for oid in repo.all_noted_commits()? {
        if let Some(r) = get(repo, oid) {
            if r.intent_id == intent_id {
                out.push((oid, r));
            }
        }
    }
    Ok(out)
}

/// Resolve a revision or an intent id (`int_…`) to a commit.
pub fn resolve(repo: &Repo, rev: &str) -> Result<Oid> {
    if rev.starts_with("int_") {
        let mut hits = find_by_intent(repo, rev)?;
        // Prefer the original realisation (operation == commit), else the first.
        hits.sort_by_key(|(_, r)| r.provenance.operation != "commit");
        return hits
            .first()
            .map(|(o, _)| *o)
            .with_context(|| format!("no commit realises intent {rev}"));
    }
    repo.resolve(rev)
}

/// The record for a commit; if the commit was made with plain git, analyze it
/// now (method recorded in provenance) and store the result.
pub fn record_for(
    repo: &Repo,
    provider: Option<&dyn Provider>,
    oid: Oid,
) -> Result<SemanticChange> {
    if let Some(r) = get(repo, oid) {
        return Ok(r);
    }
    let commit = repo.git.find_commit(oid)?;
    let message = commit.message().unwrap_or("").to_string();
    let d = repo.diff_commit(oid)?;
    let a = analyze::analyze(provider, None, &message, &d, false)?;
    let mut rec = analyze::build_record(a, &d, "annotate");
    rec.realization = realization(repo, oid)?;
    put(repo, oid, &rec)?;
    Ok(rec)
}
