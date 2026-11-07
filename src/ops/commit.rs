//! Semantic commit (with requirement proofs) and semantic diff.

use super::Session;
use crate::analyze;
use crate::model::{IntentDraft, RequirementDraft, Retirement, SemanticChange};
use crate::requirements::{self, Runner, TempTree};
use crate::store;
use anyhow::{bail, Result};
use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct CommitOptions {
    pub message: Option<String>,
    pub intent: Option<IntentDraft>,
    /// Commit only what is staged (default: stage everything like `git add -A`).
    pub staged_only: bool,
    pub require_llm: bool,
    pub dry_run: bool,
    /// Author requirements (`--require` …).
    pub requirements: Vec<RequirementDraft>,
    pub retires: Vec<Retirement>,
    /// Run the full ledger before committing.
    pub check_ledger: bool,
    /// Override `requirements.prove` (warn | require | off).
    pub prove: Option<String>,
    /// Requirement ids allowed to fail after the change.
    pub allow_failing: Vec<String>,
    /// The intent/requirements came through an agent surface (MCP/HTTP): their
    /// command checks are stored with origin `agent`, never `author` (ADR 0014 §2).
    pub agent_supplied: bool,
}

pub struct CommitResult {
    pub commit: Option<String>,
    pub record: SemanticChange,
    /// Set when the commit was rejected (nothing was changed).
    pub rejected: Option<String>,
    pub ledger_results: Vec<requirements::ReqResult>,
}

pub fn commit(s: &Session, o: &CommitOptions) -> Result<CommitResult> {
    let repo = &s.repo;
    let head = repo.head_commit()?;
    let head_id = head.as_ref().map(|c| c.id());
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

    // Retirements must name live requirements and give a reason.
    let ledger = match head_id {
        Some(h) => requirements::ledger(repo, h)?,
        None => vec![],
    };
    for r in &o.retires {
        if r.reason.trim().is_empty() {
            bail!("--retire {} needs a reason", r.id);
        }
        if !ledger.iter().any(|l| l.req.id == r.id) {
            bail!(
                "cannot retire {}: not a live requirement at HEAD (see `gitent check`)",
                r.id
            );
        }
    }

    let message = o.message.clone().unwrap_or_default();
    let a = analyze::analyze(s.provider(), o.intent.clone(), &message, &d, o.require_llm)?;
    let mut rec = analyze::build_record_with(a, &d, "commit", &o.requirements, &o.retires);
    if o.agent_supplied {
        for r in rec.requirements.iter_mut() {
            if r.origin == "author" && r.check.is_some() {
                r.origin = "agent".into();
            }
        }
    }
    if o.dry_run {
        if !o.staged_only {
            repo.git.index()?.read(true)?;
        }
        return Ok(CommitResult {
            commit: None,
            record: rec,
            rejected: None,
            ledger_results: vec![],
        });
    }

    let mut msg = analyze::commit_message(&rec, o.message.as_deref());
    if !s.cfg.commit.intent_trailer {
        // The note already links commit and record; some teams keep messages clean.
        msg = msg
            .trim_end()
            .trim_end_matches(&format!("Gitent-Intent: {}", rec.intent_id))
            .trim_end()
            .to_string()
            + "\n";
    }
    let parents: Vec<git2::Oid> = head_id.into_iter().collect();
    // Create the commit object first; the branch only moves if proofs pass.
    let oid = repo.commit(None, &msg, tree_oid, &parents)?;

    let prove = o
        .prove
        .clone()
        .unwrap_or_else(|| s.cfg.requirements.prove.clone());
    let mut problems = vec![];
    if prove != "off" {
        let rep = requirements::prove(repo, &s.cfg, &mut rec.requirements, oid, head_id, false)?;
        for (id, detail) in &rep.after_failures {
            let req = rec.requirements.iter().find(|r| &r.id == id);
            let allowed = o
                .allow_failing
                .iter()
                .any(|a| a == id || a == "all" || req.map(|r| &r.test == a).unwrap_or(false));
            if !allowed {
                let st = req.map(|r| r.statement.clone()).unwrap_or_default();
                problems.push(format!(
                    "requirement {id} \"{st}\" fails on the new commit: {}",
                    first_lines(detail, 8)
                ));
            }
        }
        for id in &rep.non_discriminating {
            let st = rec
                .requirements
                .iter()
                .find(|r| &r.id == id)
                .map(|r| r.statement.clone())
                .unwrap_or_default();
            let w = format!("non_discriminating:{id} \"{st}\" already passes without this change");
            if prove == "require" {
                problems.push(w);
            } else {
                rec.provenance.analysis.warnings.push(w);
            }
        }
    }

    // Editing a test that a live requirement is bound to changes what the requirement means.
    let retired_ids: HashSet<&str> = o.retires.iter().map(|r| r.id.as_str()).collect();
    let changed_paths: Vec<String> = d
        .files
        .iter()
        .map(|f| f.path.clone())
        .chain(d.files.iter().filter_map(|f| f.old_path.clone()))
        .collect();
    let touched = requirements::modified_tests(repo, head_tree.as_ref(), &tree, &changed_paths);
    let strict_ledger = o.check_ledger || s.cfg.requirements.check_ledger_on_commit;
    for l in &ledger {
        if l.req.kind == crate::model::ReqKind::Test
            && touched.contains(&l.req.test)
            && !retired_ids.contains(l.req.id.as_str())
        {
            let w = format!(
                "modifies_bound_test:{} test `{}` backs live requirement \"{}\" (from \"{}\"); retire it (--retire {} \"<reason>\") if its meaning changes",
                l.req.id, l.req.test, l.req.statement, l.title, l.req.id
            );
            if strict_ledger {
                problems.push(w);
            } else {
                rec.provenance.analysis.warnings.push(w);
            }
        }
    }

    let mut ledger_results = vec![];
    if o.check_ledger || s.cfg.requirements.check_ledger_on_commit {
        let retired: HashSet<&str> = o.retires.iter().map(|r| r.id.as_str()).collect();
        let live: Vec<_> = ledger
            .into_iter()
            .filter(|l| !retired.contains(l.req.id.as_str()))
            .collect();
        if live.iter().any(|l| l.req.kind.executable()) {
            let t = TempTree::new(repo, oid, "ledger")?;
            ledger_results = Runner::new(&t.path, &s.cfg).run_all(&live);
            for r in ledger_results.iter().filter(|r| r.status == "fail") {
                problems.push(format!(
                    "live requirement {} \"{}\" (from \"{}\") is broken by this commit; fix it or retire it with --retire {} \"<reason>\"",
                    r.id, r.statement, r.intent_title, r.id
                ));
            }
        }
    }

    if !problems.is_empty() {
        return Ok(CommitResult {
            commit: None,
            record: rec,
            rejected: Some(problems.join("\n")),
            ledger_results,
        });
    }

    // Move the branch (compare-and-swap against the HEAD we started from).
    let refname = repo.head_ref_name()?;
    match head_id {
        Some(h) => {
            repo.git
                .reference_matching(&refname, oid, true, h, "gitent commit")?;
        }
        None => {
            repo.git.reference(&refname, oid, false, "gitent commit")?;
        }
    }
    rec.realization = store::realization(repo, oid)?;
    store::put(repo, oid, &rec)?;
    Ok(CommitResult {
        commit: Some(oid.to_string()),
        record: rec,
        rejected: None,
        ledger_results,
    })
}

fn first_lines(s: &str, n: usize) -> String {
    s.lines().take(n).collect::<Vec<_>>().join("\n")
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
