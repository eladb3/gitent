//! Build operations (cherry-pick, revert, merge, rebase) as step lists for the engine.

use super::{engine, new_op_id, OpState, Operation, Session, Step, StepKind};
use anyhow::{bail, Result};
use git2::Oid;

#[derive(Debug, Clone, Default)]
pub struct OpOptions {
    pub strict: bool,
    pub force: bool,
    /// Skip the deterministic git attempt.
    pub no_git: bool,
}

fn base_op(
    s: &Session,
    kind: &str,
    start: Oid,
    steps: Vec<Step>,
    o: &OpOptions,
) -> Result<Operation> {
    let branch_ref = s.repo.head_ref_name()?;
    let head = s.repo.head_commit()?.map(|c| c.id().to_string());
    let id = new_op_id();
    let worktree = s.repo.worktree_root(&s.cfg.executor.worktree_root).join(&id);
    Ok(Operation {
        id,
        kind: kind.into(),
        state: OpState::Running,
        branch_ref,
        start_head: head,
        tip: start.to_string(),
        worktree: worktree.to_string_lossy().to_string(),
        executor: s.cfg.executor.kind.clone(),
        steps,
        current: 0,
        strict: o.strict,
        force: o.force,
        use_git: !o.no_git,
        warnings: vec![],
        error: None,
        result_commit: None,
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}

fn head(s: &Session) -> Result<Oid> {
    match s.repo.head_commit()? {
        Some(c) => Ok(c.id()),
        None => bail!("the current branch has no commits yet"),
    }
}

pub fn cherry_pick(s: &Session, revs: &[String], o: &OpOptions) -> Result<Operation> {
    let h = head(s)?;
    let mut steps = vec![];
    for r in revs {
        let oid = crate::store::resolve(&s.repo, r)?;
        if s.repo.git.graph_descendant_of(h, oid)? || h == oid {
            bail!("{r} is already part of the current branch");
        }
        steps.push(Step::new(StepKind::Apply {
            source: oid.to_string(),
        }));
    }
    if steps.is_empty() {
        bail!("nothing to cherry-pick");
    }
    engine::start(s, base_op(s, "cherry-pick", h, steps, o)?)
}

pub fn revert(s: &Session, rev: &str, o: &OpOptions) -> Result<Operation> {
    let h = head(s)?;
    let oid = crate::store::resolve(&s.repo, rev)?;
    if !(s.repo.git.graph_descendant_of(h, oid)? || h == oid) {
        bail!("{rev} is not part of the current branch");
    }
    engine::start(
        s,
        base_op(
            s,
            "revert",
            h,
            vec![Step::new(StepKind::Revert {
                source: oid.to_string(),
            })],
            o,
        )?,
    )
}

/// Fast-forward-only cases return `Ok(None)` after moving the branch.
pub fn merge(s: &Session, branch: &str, o: &OpOptions) -> Result<serde_json::Value> {
    let h = head(s)?;
    let theirs = s.repo.resolve(branch)?;
    let base = s.repo.merge_base(h, theirs)?;
    if base == theirs {
        return Ok(
            serde_json::json!({"state": "up_to_date", "message": format!("already up to date with {branch}")}),
        );
    }
    if base == h {
        let dirty = s.repo.dirty_paths()?;
        if !dirty.is_empty() {
            bail!(
                "working tree has uncommitted changes ({}); refusing to fast-forward",
                dirty.join(", ")
            );
        }
        let refname = s.repo.head_ref_name()?;
        s.repo.finalize_branch(&refname, Some(h), theirs)?;
        return Ok(
            serde_json::json!({"state": "fast_forward", "result_commit": theirs.to_string()}),
        );
    }
    let step = Step::new(StepKind::Merge {
        theirs: theirs.to_string(),
        theirs_name: branch.into(),
        base: base.to_string(),
    });
    let op = engine::start(s, base_op(s, "merge", h, vec![step], o)?)?;
    Ok(op.summary())
}

pub fn rebase(s: &Session, onto: &str, o: &OpOptions) -> Result<serde_json::Value> {
    let h = head(s)?;
    let onto_oid = s.repo.resolve(onto)?;
    let base = s.repo.merge_base(h, onto_oid)?;
    if base == onto_oid {
        return Ok(
            serde_json::json!({"state": "up_to_date", "message": format!("already based on {onto}")}),
        );
    }
    let commits = s.repo.commits_between(base, h)?;
    if commits.is_empty() || base == h {
        let refname = s.repo.head_ref_name()?;
        s.repo.finalize_branch(&refname, Some(h), onto_oid)?;
        return Ok(
            serde_json::json!({"state": "fast_forward", "result_commit": onto_oid.to_string()}),
        );
    }
    let steps = commits
        .into_iter()
        .map(|c| {
            Step::new(StepKind::Apply {
                source: c.to_string(),
            })
        })
        .collect();
    let op = engine::start(s, base_op(s, "rebase", onto_oid, steps, o)?)?;
    Ok(op.summary())
}
