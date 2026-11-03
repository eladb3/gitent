//! `gitent verify`: validate an existing result as a move of a change (ADR 0011).

use super::engine::{load_ctx, prepare_step, validate_candidate};
use super::{Session, Step, StepKind};
use crate::model::*;
use crate::repo::short;
use crate::requirements::TempTree;
use crate::store;
use anyhow::{bail, Result};
use git2::Oid;
use serde_json::{json, Value};

#[derive(Debug, Clone, Default)]
pub struct VerifyOptions {
    pub source: String,
    pub result: Option<String>,
    pub onto: Option<String>,
    pub working_tree: bool,
    /// apply | revert | merge
    pub kind: String,
    pub record: bool,
}

pub fn verify(s: &Session, o: &VerifyOptions) -> Result<Value> {
    let repo = &s.repo;
    let source = store::resolve(repo, &o.source)?;
    let kind_name = if o.kind.is_empty() {
        "apply"
    } else {
        o.kind.as_str()
    };

    // The result to verify: a commit, or a snapshot of the working tree.
    let (result_commit, result_tree, is_snapshot) = if o.working_tree {
        if o.result.is_some() {
            bail!("--working-tree and <result> are mutually exclusive");
        }
        let head = repo
            .head_commit()?
            .ok_or_else(|| anyhow::anyhow!("no HEAD to compare the working tree with"))?;
        let mut idx = repo.git.index()?;
        idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        idx.update_all(["*"].iter(), None)?;
        let tree = idx.write_tree()?;
        idx.read(true)?; // discard in-memory changes: the user's index is untouched
                         // A dangling commit so the snapshot can be checked out in a temporary worktree.
        let c = repo.commit(
            None,
            "gitent verify: working tree snapshot",
            tree,
            &[head.id()],
        )?;
        (c, tree, true)
    } else {
        let c = repo.resolve(o.result.as_deref().unwrap_or("HEAD"))?;
        (c, repo.git.find_commit(c)?.tree_id(), false)
    };
    let result = repo.git.find_commit(result_commit)?;

    let base = match &o.onto {
        Some(b) => repo.resolve(b)?,
        None => {
            if result.parent_count() == 0 {
                bail!("the result has no parent; pass --onto <base>");
            }
            result.parent_id(0)?
        }
    };
    if base == result_commit {
        bail!("base and result are the same commit");
    }
    let kind = match kind_name {
        "apply" => StepKind::Apply {
            source: source.to_string(),
        },
        "revert" => StepKind::Revert {
            source: source.to_string(),
        },
        "merge" => StepKind::Merge {
            theirs: source.to_string(),
            theirs_name: o.source.clone(),
            base: repo.merge_base(base, source)?.to_string(),
        },
        other => bail!("unknown --kind {other} (apply | revert | merge)"),
    };

    let ctx = load_ctx(s, &kind, base)?;
    let mut step = Step::new(kind.clone());
    let base_tree = repo.git.find_commit(base)?.tree()?;
    let branch = repo.head_ref_name().unwrap_or_else(|_| "HEAD".into());
    prepare_step(
        s,
        &mut step,
        &ctx,
        base,
        branch.trim_start_matches("refs/heads/"),
        crate::preserve::test_names(repo, &base_tree),
    )?;

    let tmp = TempTree::new(repo, result_commit, "verify")?;
    let report = validate_candidate(s, &step, base, result_tree, &tmp.path)?;
    drop(tmp);

    let mut out = json!({
        "verified": report.passed,
        "state": if report.passed { "succeeded" } else { "failed" },
        "kind": kind_name,
        "source": source.to_string(),
        "base": base.to_string(),
        "result": if is_snapshot { Value::String("working-tree".into()) } else { Value::String(result_commit.to_string()) },
        "requirements_checked": step.requirements.len(),
        "validation": report,
    });

    if o.record {
        if is_snapshot {
            bail!("--record needs a commit, not --working-tree");
        }
        if !report.passed {
            out["recorded"] = json!(false);
        } else if store::get(repo, result_commit).is_some() {
            out["recorded"] = json!(false);
            out["record_note"] = json!("result already has a semantic record; left unchanged");
        } else {
            let rec =
                record_for_verified(s, &kind, &ctx.records, source, base, result_commit, &report)?;
            store::put(repo, result_commit, &rec)?;
            out["recorded"] = json!(true);
            out["record"] = serde_json::to_value(&rec)?;
        }
    }
    if !report.passed {
        out["error"] = json!(format!(
            "{} is not a valid {kind_name} of {} onto {}:\n{}",
            if is_snapshot {
                "the working tree".to_string()
            } else {
                short(&result_commit.to_string())
            },
            short(&source.to_string()),
            short(&base.to_string()),
            crate::validate::feedback(&report)
        ));
    }
    Ok(out)
}

fn record_for_verified(
    s: &Session,
    kind: &StepKind,
    sources: &[(Oid, SemanticChange)],
    source: Oid,
    base: Oid,
    result: Oid,
    report: &ValidationReport,
) -> Result<SemanticChange> {
    let repo = &s.repo;
    let bt = repo.git.find_commit(base)?.tree()?;
    let rt = repo.git.find_commit(result)?.tree()?;
    let d = repo.diff_trees(Some(&bt), &rt)?;
    let src = sources.first().map(|(_, r)| r.clone()).unwrap_or_default();
    let mut rec = match kind {
        StepKind::Apply { .. } => {
            let mut r = src.clone();
            for q in r.requirements.iter_mut() {
                q.rebound_from = Some(source.to_string());
                q.proof = None;
            }
            r.retires = vec![];
            r.provenance.derived_from = Some(SourceRef {
                commit: source.to_string(),
                intent_id: src.intent_id.clone(),
            });
            r
        }
        StepKind::Revert { .. } => {
            let mut r = src.clone();
            r.intent_id =
                crate::analyze::make_intent_id(&["revert", &src.intent_id, &result.to_string()]);
            r.kind = ChangeKind::Revert;
            r.title = format!("Revert \"{}\"", src.title);
            r.intent = format!(
                "Remove the behaviour introduced by {} ({}).",
                src.intent_id, src.title
            );
            r.retires = src
                .requirements
                .iter()
                .map(|q| Retirement {
                    id: q.id.clone(),
                    reason: format!("reverted \"{}\"", src.title),
                })
                .collect();
            r.requirements = vec![];
            r.provenance.reverts = Some(SourceRef {
                commit: source.to_string(),
                intent_id: src.intent_id.clone(),
            });
            r
        }
        StepKind::Merge { .. } => {
            let mut r = SemanticChange {
                schema: SCHEMA.into(),
                intent_id: crate::analyze::make_intent_id(&[
                    "merge",
                    &source.to_string(),
                    &result.to_string(),
                ]),
                kind: ChangeKind::Merge,
                title: format!("Merge {}", short(&source.to_string())),
                intent: "Compose both sides' intentions".into(),
                behavior: "All composed intentions hold simultaneously.".into(),
                ..Default::default()
            };
            r.provenance.composes = sources
                .iter()
                .map(|(o, x)| SourceRef {
                    commit: o.to_string(),
                    intent_id: x.intent_id.clone(),
                })
                .collect();
            r
        }
    };
    rec.schema = SCHEMA.into();
    rec.affected = Affected {
        files: d.files.iter().map(|f| f.path.clone()).collect(),
        symbols: crate::diff::symbols_from_patch(&d.patch),
    };
    rec.tests = crate::diff::tests_added_in_patch(&d.patch);
    rec.realization = store::realization(repo, result)?;
    rec.provenance.operation = "verify".into();
    rec.provenance.executor = "external".into();
    rec.provenance.attempts = 0;
    rec.provenance.op_id = String::new();
    rec.provenance.validation = Some(report.clone());
    rec.provenance.created_at =
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    rec.confidence = if report.checks.iter().any(|c| c.name == "tests" && c.passed) {
        0.8
    } else {
        0.5
    };
    Ok(rec)
}
