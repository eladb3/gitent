//! The step engine: deterministic attempt → agent attempts → validation →
//! commit-in-worktree → compare-and-swap finalize (ADR 0003).

use super::brief::{self, BriefInput};
use super::{Attempt, OpState, Operation, Session, Step, StepKind, StepStatus};
use crate::agent::{self, ExecutorChoice, Task};
use crate::diff;
use crate::llm::{extract_json, Message};
use crate::model::*;
use crate::repo::{short, MergeOutcome, Repo};
use crate::requirements;
use crate::store;
use crate::validate;
use anyhow::{Context, Result};
use git2::Oid;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Records and diffs a step needs (loaded on demand, not persisted).
pub(crate) struct StepCtx {
    pub(crate) records: Vec<(Oid, SemanticChange)>,
    preserve: Vec<(Oid, SemanticChange)>,
    reference_diff: String,
    reference_label: String,
}

enum Outcome {
    Done,
    AwaitHost,
    Failed(String),
}

/// Create the worktree and run an operation built by `plan`.
pub fn start(s: &Session, mut op: Operation) -> Result<Operation> {
    let dirty = s.repo.dirty_paths()?;
    if !dirty.is_empty() {
        anyhow::bail!(
            "working tree has uncommitted changes ({}); commit or stash them first — gitent never overwrites local changes",
            dirty.join(", ")
        );
    }
    let tip = Oid::from_str(&op.tip)?;
    s.repo
        .add_worktree(Path::new(&op.worktree), tip)
        .context("creating isolated worktree")?;
    s.save_op(&op)?;
    drive(s, &mut op, false);
    Ok(op)
}

/// `gitent continue`: validate the host agent's work and keep going.
pub fn resume(s: &Session, id: Option<&str>) -> Result<Operation> {
    let mut op = s.pick_open_op(id)?;
    if op.state != OpState::AwaitingHost {
        anyhow::bail!(
            "operation {} is {:?}, not awaiting the host agent",
            op.id,
            op.state
        );
    }
    op.state = OpState::Running;
    drive(s, &mut op, true);
    Ok(op)
}

/// Abort an open operation, or clean up a failed one whose worktree was kept.
pub fn abort(s: &Session, id: Option<&str>) -> Result<Operation> {
    let mut op = match id {
        Some(i) => s.load_op(i)?,
        None => s
            .list_ops()?
            .into_iter()
            .rfind(|o| {
                o.state.is_open() || (o.state == OpState::Failed && Path::new(&o.worktree).exists())
            })
            .ok_or_else(|| anyhow::anyhow!("no operation to abort"))?,
    };
    if op.state == OpState::Succeeded {
        anyhow::bail!("operation {} already succeeded", op.id);
    }
    s.repo
        .remove_worktree_in(&op_worktree_root(s), Path::new(&op.worktree))?;
    op.state = OpState::Aborted;
    op.error = Some("aborted by user".into());
    s.save_op(&op)?;
    Ok(op)
}

/// Where this repository's operation worktrees live (only these are ever deleted).
fn op_worktree_root(s: &Session) -> PathBuf {
    s.repo.worktree_root(&s.cfg.executor.worktree_root)
}

fn drive(s: &Session, op: &mut Operation, mut resuming: bool) {
    let result = (|| -> Result<Option<String>> {
        while op.current < op.steps.len() {
            let i = op.current;
            match run_step(s, op, i, resuming)? {
                Outcome::Done => {
                    op.current += 1;
                    resuming = false;
                    s.save_op(op)?;
                }
                Outcome::AwaitHost => {
                    op.state = OpState::AwaitingHost;
                    s.save_op(op)?;
                    return Ok(None);
                }
                Outcome::Failed(why) => return Ok(Some(why)),
            }
        }
        finalize(s, op)?;
        Ok(None)
    })();
    match result {
        Ok(None) => {}
        Ok(Some(why)) => fail(op, why),
        Err(e) => fail(op, format!("{e:#}")),
    }
    let _ = s.save_op(op);
}

fn fail(op: &mut Operation, why: String) {
    op.state = OpState::Failed;
    op.error = Some(format!(
        "{why}\nThe branch was not changed. The worktree is kept for inspection at {} (remove with `gitent abort {}`).",
        op.worktree, op.id
    ));
}

fn finalize(s: &Session, op: &mut Operation) -> Result<()> {
    let tip = Oid::from_str(&op.tip)?;
    if op.strict && !op.warnings.is_empty() {
        anyhow::bail!(
            "--strict: refusing to finalize with warnings: {}",
            op.warnings.join("; ")
        );
    }
    let expected = op.start_head.as_deref().map(Oid::from_str).transpose()?;
    s.repo.finalize_branch(&op.branch_ref, expected, tip)?;
    // The branch already moved: a cleanup refusal must not fail the operation.
    if let Err(e) = s
        .repo
        .remove_worktree_in(&op_worktree_root(s), Path::new(&op.worktree))
    {
        op.warnings.push(format!("{e:#}"));
    }
    op.state = OpState::Succeeded;
    op.result_commit = Some(op.tip.clone());
    Ok(())
}

pub(crate) fn load_ctx(s: &Session, kind: &StepKind, tip: Oid) -> Result<StepCtx> {
    let p = s.provider();
    Ok(match kind {
        StepKind::Apply { source } | StepKind::Revert { source } => {
            let oid = Oid::from_str(source)?;
            let rec = store::record_for(&s.repo, p, oid)?;
            let d = s.repo.diff_commit(oid)?;
            let label = if matches!(kind, StepKind::Apply { .. }) {
                "Original change"
            } else {
                "Change being reverted"
            };
            StepCtx {
                records: vec![(oid, rec)],
                preserve: vec![],
                reference_diff: d.patch,
                reference_label: format!("{label} ({})", short(source)),
            }
        }
        StepKind::Merge {
            theirs,
            theirs_name,
            base,
        } => {
            let theirs_oid = Oid::from_str(theirs)?;
            let base_oid = Oid::from_str(base)?;
            let mut records = vec![];
            for c in s.repo.commits_between(base_oid, theirs_oid)? {
                records.push((c, store::record_for(&s.repo, p, c)?));
            }
            let mut preserve = vec![];
            for c in s.repo.commits_between(base_oid, tip)? {
                preserve.push((c, store::record_for(&s.repo, p, c)?));
            }
            let bt = s.repo.git.find_commit(base_oid)?.tree()?;
            let tt = s.repo.git.find_commit(theirs_oid)?.tree()?;
            let d = s.repo.diff_trees(Some(&bt), &tt)?;
            StepCtx {
                records,
                preserve,
                reference_diff: d.patch,
                reference_label: format!("Changes on `{theirs_name}` since the merge base"),
            }
        }
    })
}

pub(crate) fn goal_for(kind: &StepKind, ctx: &StepCtx, branch: &str) -> String {
    match kind {
        StepKind::Apply { .. } => {
            let r = &ctx.records[0].1;
            format!(
                "Re-implement the intention **{}** (`{}`) on top of the current code of `{branch}`.\n\nWhy: {}\n\nExpected behaviour: {}",
                r.title,
                r.intent_id,
                r.intent,
                if r.behavior.is_empty() { "(see intent and reference diff)" } else { &r.behavior }
            )
        }
        StepKind::Revert { .. } => {
            let r = &ctx.records[0].1;
            format!(
                "Remove the behaviour introduced by the intention **{}** (`{}`) from the current code of `{branch}`, \
                 restoring the behaviour from before it, while keeping every other (including later) change working.\n\n\
                 The intention being removed: {}\nIts behaviour: {}",
                r.title, r.intent_id, r.intent, r.behavior
            )
        }
        StepKind::Merge { theirs_name, .. } => {
            let t: Vec<String> = ctx
                .records
                .iter()
                .map(|(_, r)| format!("- {} (`{}`)", r.title, r.intent_id))
                .collect();
            let o: Vec<String> = ctx
                .preserve
                .iter()
                .map(|(_, r)| format!("- {} (`{}`)", r.title, r.intent_id))
                .collect();
            format!(
                "Integrate the intentions from `{theirs_name}` into the current code of `{branch}`. \
                 The current code already realises our side's intentions; after your change BOTH sides' intentions must hold.\n\n\
                 Theirs:\n{}\n\nOurs (already present):\n{}",
                t.join("\n"),
                if o.is_empty() { "- (none)".into() } else { o.join("\n") }
            )
        }
    }
}

/// Fill in what a step must satisfy: goal, test guards, requirements (ADR 0008 §7).
/// Shared by operations and `gitent verify` (ADR 0011).
pub(crate) fn prepare_step(
    s: &Session,
    step: &mut Step,
    ctx: &StepCtx,
    tip: Oid,
    branch: &str,
    tests_before: BTreeSet<String>,
) -> Result<()> {
    let kind = step.kind.clone();
    step.tests_before = tests_before.into_iter().collect();
    step.goal = goal_for(&kind, ctx, branch);
    match &kind {
        StepKind::Apply { .. } => {
            step.required_tests = ctx.records[0].1.tests.clone();
            // Tests the change itself deletes may disappear on the target too.
            step.removable_tests = diff::tests_removed_in_patch(&ctx.reference_diff);
        }
        StepKind::Revert { .. } => step.removable_tests = ctx.records[0].1.tests.clone(),
        StepKind::Merge { .. } => {
            let mut t: BTreeSet<String> = BTreeSet::new();
            for (_, r) in &ctx.records {
                t.extend(r.tests.iter().cloned());
            }
            // Tests removed on their side may legitimately disappear.
            step.removable_tests = diff::tests_removed_in_patch(&ctx.reference_diff);
            step.required_tests = t.into_iter().collect();
        }
    }
    let tip_ledger = requirements::ledger(&s.repo, tip)?;
    let (reqs, retiring) = match &kind {
        StepKind::Apply { source } => {
            let src = requirements::live_from_record(&ctx.records[0].1, source);
            (requirements::union(src, tip_ledger), vec![])
        }
        StepKind::Revert { .. } => {
            let ids: std::collections::HashSet<String> = ctx.records[0]
                .1
                .requirements
                .iter()
                .map(|r| r.id.clone())
                .collect();
            let (gone, keep): (Vec<_>, Vec<_>) = tip_ledger
                .into_iter()
                .partition(|l| ids.contains(&l.req.id));
            (keep, gone)
        }
        StepKind::Merge { theirs, .. } => {
            drop(tip_ledger);
            (
                requirements::merge_ledger(&s.repo, tip, Oid::from_str(theirs)?)?,
                vec![],
            )
        }
    };
    step.requirements = reqs;
    step.retiring = retiring;
    Ok(())
}

fn run_step(s: &Session, op: &mut Operation, i: usize, resuming: bool) -> Result<Outcome> {
    let tip = Oid::from_str(&op.tip)?;
    let wt_path = PathBuf::from(&op.worktree);
    let wt = Repo::open_exact(&wt_path).context("opening worktree")?;
    let kind = op.steps[i].kind.clone();
    let ctx = load_ctx(s, &kind, tip)?;
    let branch = op.branch_ref.trim_start_matches("refs/heads/").to_string();
    let max_attempts = s.cfg.validation.max_attempts.max(1);
    let exec_cfg = {
        let mut c = s.cfg.clone();
        c.executor.kind = op.executor.clone();
        c
    };

    if !resuming {
        // ---- prepare ----
        Repo::reset_worktree(&wt, tip, None)?;
        let tip_tree = s.repo.git.find_commit(tip)?.tree()?;
        let tests_before = crate::preserve::test_names(&s.repo, &tip_tree);
        prepare_step(s, &mut op.steps[i], &ctx, tip, &branch, tests_before)?;

        // ---- 1. deterministic attempt ----
        let mut starting_point = String::from("The worktree is at the current tip of the branch.");
        if op.use_git {
            let outcome = match &kind {
                StepKind::Apply { source } => {
                    s.repo.cherry_pick_in_memory(Oid::from_str(source)?, tip)?
                }
                StepKind::Revert { source } => {
                    s.repo.revert_in_memory(Oid::from_str(source)?, tip)?
                }
                StepKind::Merge { theirs, .. } => {
                    s.repo.merge_in_memory(tip, Oid::from_str(theirs)?)?
                }
            };
            match outcome {
                MergeOutcome::Clean(tree) => {
                    Repo::reset_worktree(&wt, tip, Some(tree))?;
                    let (snap, report) = evaluate(s, op, i, &wt)?;
                    let passed = report.passed;
                    let feedback = validate::feedback(&report);
                    op.steps[i].attempts.push(Attempt {
                        n: 0,
                        by: "git".into(),
                        exec: None,
                        validation: report,
                    });
                    if passed {
                        commit_step(s, op, i, &ctx, snap, "git")?;
                        return Ok(Outcome::Done);
                    }
                    starting_point = format!(
                        "Git applied the change textually WITHOUT conflicts, and the worktree contains that result, \
                         but it FAILED validation — the textual result is semantically wrong for this code base. \
                         Fix it (or redo it) so the intention is realised correctly.\n\nValidation failures:\n{feedback}"
                    );
                }
                MergeOutcome::Conflicts(paths, partial) => {
                    if let Some(t) = partial {
                        Repo::reset_worktree(&wt, tip, Some(t))?;
                    }
                    op.steps[i].attempts.push(Attempt {
                        n: 0,
                        by: "git".into(),
                        exec: None,
                        validation: ValidationReport {
                            passed: false,
                            checks: vec![CheckResult {
                                name: "git_apply".into(),
                                passed: false,
                                detail: format!("textual conflicts in: {}", paths.join(", ")),
                                ..Default::default()
                            }],
                            warnings: vec![],
                        },
                    });
                    starting_point = format!(
                        "A plain git {} CONFLICTS in: {}. All NON-conflicting parts of the change are already applied in the worktree; \
                         the conflicting files are exactly as in the current code (no conflict markers). \
                         Re-implement the intention there by understanding the current code.",
                        op.kind,
                        paths.join(", ")
                    );
                }
            }
        }

        // ---- merge compatibility check (before spending agent effort) ----
        if matches!(kind, StepKind::Merge { .. }) && !op.force {
            if let Some(report) = compatibility(s, &ctx)? {
                let incompatible = report["compatible"] == false
                    && report["confidence"].as_f64().unwrap_or(0.0)
                        >= s.cfg.validation.verifier_threshold;
                op.steps[i].compatibility = Some(report.clone());
                if incompatible {
                    op.steps[i].status = StepStatus::Failed;
                    return Ok(Outcome::Failed(format!(
                        "the intentions on both sides are incompatible (needs a human decision; re-run with --force to try anyway): {}",
                        report["conflicts"]
                    )));
                }
            }
        }

        write_brief(s, op, i, &ctx, &starting_point, &[])?;
    } else {
        // ---- host finished an attempt: validate it ----
        let (snap, report) = evaluate(s, op, i, &wt)?;
        let n = op.steps[i].agent_attempts() + 1;
        let passed = report.passed;
        op.steps[i].attempts.push(Attempt {
            n,
            by: "host".into(),
            exec: None,
            validation: report,
        });
        if passed {
            commit_step(s, op, i, &ctx, snap, "host")?;
            return Ok(Outcome::Done);
        }
        if n >= max_attempts {
            op.steps[i].status = StepStatus::Failed;
            return Ok(Outcome::Failed(format!(
                "validation failed after {n} attempt(s):\n{}",
                last_feedback(&op.steps[i])
            )));
        }
        let fb = feedbacks(&op.steps[i]);
        write_brief(
            s,
            op,
            i,
            &ctx,
            "The worktree contains your previous attempt.",
            &fb,
        )?;
        op.steps[i].status = StepStatus::AwaitingHost;
        return Ok(Outcome::AwaitHost);
    }

    // ---- 2. agent attempts ----
    let executor = agent::from_config(&exec_cfg, s.provider.clone())?;
    let exec = match executor {
        ExecutorChoice::Host => {
            op.steps[i].status = StepStatus::AwaitingHost;
            return Ok(Outcome::AwaitHost);
        }
        ExecutorChoice::Run(e) => e,
    };
    for n in 1..=max_attempts {
        let brief_path = PathBuf::from(op.steps[i].brief_path.clone().unwrap_or_default());
        let task = Task {
            op_id: op.id.clone(),
            attempt: n,
            worktree: wt_path.clone(),
            brief: std::fs::read_to_string(&brief_path)?,
            brief_path,
        };
        // ADR 0014 §8: the agent must not touch the shared hooks/ or config.
        let gitdir_before = super::containment::GitDirSnapshot::take(&s.repo.git_dir);
        let report = exec.execute(&task);
        let mut tampered = gitdir_before.changes();
        if !tampered.is_empty() {
            gitdir_before
                .restore()
                .context("restoring .git/hooks and .git/config after the agent changed them")?;
        }
        let report = report?;
        let evaluated = evaluate(s, op, i, &wt);
        // Validation runs the agent's code (its tests) too.
        let during_tests = gitdir_before.changes();
        if !during_tests.is_empty() {
            gitdir_before
                .restore()
                .context("restoring .git/hooks and .git/config after validation changed them")?;
            tampered.extend(
                during_tests
                    .into_iter()
                    .map(|c| format!("{c} (during validation)")),
            );
        }
        let (snap, mut validation) = evaluated?;
        if !tampered.is_empty() {
            validation.passed = false;
            validation.checks.insert(
                0,
                CheckResult {
                    name: "git_dir_integrity".into(),
                    passed: false,
                    detail: format!(
                        "agent modified .git/hooks or .git/config; restored ({}). \
                         Never change files in the repository's git directory.",
                        tampered.join(", ")
                    ),
                    ..Default::default()
                },
            );
        }
        if !report.success {
            validation.passed = false;
            validation.checks.insert(
                0,
                CheckResult {
                    name: "executor".into(),
                    passed: false,
                    detail: report.summary.clone(),
                    ..Default::default()
                },
            );
        }
        let passed = validation.passed;
        op.steps[i].attempts.push(Attempt {
            n,
            by: exec.name(),
            exec: Some(report),
            validation,
        });
        s.save_op(op)?;
        if passed {
            commit_step(s, op, i, &ctx, snap, &exec.name())?;
            return Ok(Outcome::Done);
        }
        if n < max_attempts {
            let fb = feedbacks(&op.steps[i]);
            write_brief(
                s,
                op,
                i,
                &ctx,
                "The worktree contains your previous attempt.",
                &fb,
            )?;
        }
    }
    op.steps[i].status = StepStatus::Failed;
    Ok(Outcome::Failed(format!(
        "rejected: validation failed after {max_attempts} agent attempt(s):\n{}",
        last_feedback(&op.steps[i])
    )))
}

fn feedbacks(step: &Step) -> Vec<String> {
    step.attempts
        .iter()
        .filter(|a| a.by != "git")
        .map(|a| validate::feedback(&a.validation))
        .collect()
}

fn last_feedback(step: &Step) -> String {
    step.attempts
        .last()
        .map(|a| validate::feedback(&a.validation))
        .unwrap_or_default()
}

fn write_brief(
    s: &Session,
    op: &mut Operation,
    i: usize,
    ctx: &StepCtx,
    starting_point: &str,
    feedback: &[String],
) -> Result<()> {
    let step = &op.steps[i];
    let records: Vec<SemanticChange> = ctx.records.iter().map(|(_, r)| r.clone()).collect();
    let preserve: Vec<SemanticChange> = ctx.preserve.iter().map(|(_, r)| r.clone()).collect();
    let text = brief::render(&BriefInput {
        op_kind: &op.kind,
        step_no: i + 1,
        step_count: op.steps.len(),
        worktree: Path::new(&op.worktree),
        goal: &step.goal,
        records: &records,
        preserve: &preserve,
        reference_diff: &ctx.reference_diff,
        reference_label: &ctx.reference_label,
        starting_point,
        required_tests: &step.required_tests,
        removable_tests: &step.removable_tests,
        test_command: &s.cfg.validation.test_command,
        test_selector: &s.cfg.validation.test_selector,
        requirements: &step.requirements,
        retiring: &step.retiring,
        feedback,
    });
    let dir = s.ops_dir().join(&op.id);
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(format!("brief-step{}.md", i + 1));
    std::fs::write(&p, text)?;
    op.steps[i].brief_path = Some(p.to_string_lossy().to_string());
    Ok(())
}

/// Snapshot the worktree (before running tests, so test byproducts are never
/// committed) and validate it.
fn evaluate(s: &Session, op: &Operation, i: usize, wt: &Repo) -> Result<(Oid, ValidationReport)> {
    let snap = wt.stage_all()?;
    let tip = Oid::from_str(&op.tip)?;
    let mut report = validate_candidate(s, &op.steps[i], tip, snap, &wt.workdir)?;
    // In an operation, findings only a human can settle (e.g. a test's expected values
    // adapted to the target's units) do not block; they are reported (ADR 0013 §1).
    for c in report.checks.iter_mut().filter(|c| !c.passed && c.review) {
        c.passed = true;
        report
            .warnings
            .push(format!("needs_review: {}: {}", c.name, c.detail));
    }
    report.passed = report.checks.iter().all(|c| c.passed);
    Ok((snap, report))
}

/// Validate a candidate tree `snap` (checked out at `workdir`) for `step`, starting from `tip`.
/// The single validation code path for operations and `gitent verify` (ADR 0011).
pub(crate) fn validate_candidate(
    s: &Session,
    step: &Step,
    tip: Oid,
    snap: Oid,
    workdir: &Path,
) -> Result<ValidationReport> {
    let tip_tree = s.repo.git.find_commit(tip)?.tree()?;
    let snap_tree = s.repo.git.find_tree(snap)?;
    let d = s.repo.diff_trees(Some(&tip_tree), &snap_tree)?;
    let before: BTreeSet<String> = step.tests_before.iter().cloned().collect();
    // Test inventories and locations come from git trees (ADR 0013 §3, §8).
    let index = crate::preserve::test_index(&s.repo, &snap_tree);
    let after: BTreeSet<String> = index.keys().cloned().collect();
    let locator = requirements::TestLocator {
        index,
        added: Some(diff::tests_added_with_paths(&d.patch).into_iter().collect()),
    };
    let path_of = |t: &str| {
        step.requirements
            .iter()
            .find(|l| l.req.kind == ReqKind::Test && l.req.test == t)
            .map(|l| l.req.test_path.clone())
            .unwrap_or_default()
    };
    let missing_required: Vec<String> = step
        .required_tests
        .iter()
        .filter_map(|t| locator.locate(t, &path_of(t)).err())
        .collect();
    let mut goal = step.goal.clone();
    if let StepKind::Merge { .. } = step.kind {
        goal.push_str("\n(For a merge, the diff below is relative to OUR side; it should add their intentions.)");
    }
    // Executable requirements first (ADR 0008 §7); judged ones go to the verifier.
    let mut runner = requirements::Runner::new(workdir, &s.cfg).with_locator(locator);
    let exec: Vec<_> = step
        .requirements
        .iter()
        .filter(|l| l.req.kind.executable())
        .cloned()
        .collect();
    let results = runner.run_all(&exec);
    let suite = runner.suite();
    let failing: Vec<_> = results.iter().filter(|r| r.status == "fail").collect();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    let judged: Vec<String> = step
        .requirements
        .iter()
        .filter(|l| !l.req.kind.executable())
        .map(|l| {
            format!(
                "- [{}] {} (from \"{}\")",
                l.req.category, l.req.statement, l.title
            )
        })
        .collect();
    if s.cfg.requirements.judge_in_operations && !judged.is_empty() {
        goal.push_str(&format!(
            "\n\nJudged requirements that must also hold:\n{}",
            judged.join("\n")
        ));
    }
    let mut vcfg = s.cfg.validation.clone();
    if !failing.is_empty() {
        vcfg.verifier = false;
    }
    let mut report = validate::validate(&validate::Ctx {
        worktree: workdir,
        diff: &d,
        tests_before: &before,
        tests_after: &after,
        required_tests: &step.required_tests,
        missing_required: &missing_required,
        suite,
        removable_tests: &step.removable_tests,
        cfg: &vcfg,
        verifier: s.provider(),
        goal: &goal,
    });
    if !exec.is_empty() {
        report.checks.insert(
            1,
            CheckResult {
                name: "requirements".into(),
                passed: failing.is_empty(),
                detail: if failing.is_empty() {
                    format!(
                        "{} executable requirement(s) satisfied{}",
                        results.len() - skipped,
                        if skipped > 0 {
                            format!(", {skipped} skipped")
                        } else {
                            String::new()
                        }
                    )
                } else {
                    failing
                        .iter()
                        .map(|r| {
                            format!(
                                "{} \"{}\" (from \"{}\"): {}",
                                r.id,
                                r.statement,
                                r.intent_title,
                                r.detail.lines().take(4).collect::<Vec<_>>().join(" | ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                },
                ..Default::default()
            },
        );
        report.passed = report.checks.iter().all(|c| c.passed);
    }
    // The target's own structure must survive (ADR 0010), and its tests must keep testing
    // (ADR 0013 §2, §6). A guard that cannot run is a review failure, not a warning.
    let guards: [(&str, Result<Option<CheckResult>>); 4] = [
        (
            "target_preserved",
            crate::preserve::check(&s.repo, &step.kind, tip, snap).map(Some),
        ),
        (
            "tests_intact",
            crate::integrity::tests_intact(&s.repo, &step.kind, tip, snap).map(Some),
        ),
        (
            "bodies_intact",
            crate::integrity::bodies_intact(&s.repo, &step.kind, tip, snap).map(Some),
        ),
        (
            "no_orphan_copy",
            crate::integrity::no_orphan_copy(&s.repo, &step.kind, tip, snap),
        ),
    ];
    for (name, g) in guards {
        match g {
            Ok(Some(c)) => report.checks.push(c),
            Ok(None) => {}
            Err(e) => report.checks.push(CheckResult {
                name: name.into(),
                passed: false,
                review: true,
                detail: format!("could not run this check: {e:#}"),
            }),
        }
    }
    report.passed = report.checks.iter().all(|c| c.passed);
    // Revert: the removed requirements should no longer hold, *with the reverted
    // change's tests present* (ADR 0013 §5): deleting the test proves nothing.
    let retiring: Vec<_> = step
        .retiring
        .iter()
        .filter(|l| l.req.kind.executable())
        .cloned()
        .collect();
    if let (false, StepKind::Revert { source }) = (retiring.is_empty(), &step.kind) {
        report
            .checks
            .push(revert_effective(s, source, tip, snap, &retiring)?);
        report.passed = report.checks.iter().all(|c| c.passed);
    }
    // Already realised (e.g. a later rebase step whose change an earlier step
    // already contains): nothing changed, but the source's executable
    // requirements all hold. Treated like git's dropped empty commit.
    if let StepKind::Apply { source } = &step.kind {
        let src_exec = results
            .iter()
            .filter(|r| &r.introduced_by == source)
            .count();
        let only_empty = report
            .checks
            .iter()
            .filter(|c| !c.passed)
            .all(|c| c.name == "non_empty")
            && report
                .checks
                .iter()
                .any(|c| c.name == "non_empty" && !c.passed);
        if only_empty && src_exec > 0 && failing.is_empty() {
            for c in report.checks.iter_mut().filter(|c| c.name == "non_empty") {
                c.passed = true;
                c.detail = format!("no change needed: the intention is already realised ({src_exec} of its executable requirement(s) pass)");
            }
            report
                .warnings
                .push(format!("already_satisfied:{}", short(source)));
            report.passed = report.checks.iter().all(|c| c.passed);
        }
    }
    Ok(report)
}

/// Run the reverted requirements on the result with the reverted change's test files overlaid.
fn revert_effective(
    s: &Session,
    source: &str,
    tip: Oid,
    snap: Oid,
    retiring: &[requirements::LiveReq],
) -> Result<CheckResult> {
    let src = s.repo.git.find_commit(Oid::from_str(source)?)?;
    let snap_commit = s
        .repo
        .commit(None, "gitent: revert check snapshot", snap, &[tip])?;
    let tmp = requirements::TempTree::new(&s.repo, snap_commit, "revert-check")?;
    let missing = |r: &requirements::ReqResult| {
        r.status == "fail"
            && (r.detail.contains("does not exist")
                || r.detail.contains("was not ported")
                || r.detail.contains("ran 0 tests"))
    };
    // 1. As the result stands (a revert may keep an adapted copy of the test).
    let first = requirements::Runner::new(&tmp.path, &s.cfg).run_all(retiring);
    // 2. Requirements whose test the result no longer has: restore the reverted change's
    //    test files and run again; deleting a test proves nothing.
    let gone: Vec<requirements::LiveReq> = retiring
        .iter()
        .filter(|l| first.iter().any(|r| r.id == l.req.id && missing(r)))
        .cloned()
        .collect();
    let mut results: Vec<requirements::ReqResult> =
        first.into_iter().filter(|r| !missing(r)).collect();
    if !gone.is_empty() && src.parent_count() > 0 {
        requirements::overlay_test_files(&s.repo, src.parent_id(0)?, src.id(), &tmp.path)?;
        results.extend(requirements::Runner::new(&tmp.path, &s.cfg).run_all(&gone));
    }
    let (mut still, mut unrun) = (vec![], vec![]);
    for r in results {
        let label = format!("{} \"{}\"", r.id, r.statement);
        if r.status == "pass" {
            still.push(label);
        } else if missing(&r) || r.status != "fail" {
            unrun.push(label);
        }
    }
    Ok(if !still.is_empty() {
        CheckResult {
            name: "revert_effective".into(),
            passed: false,
            detail: format!(
                "the reverted behaviour is still present; these requirements still pass: {}",
                still.join(", ")
            ),
            ..Default::default()
        }
    } else if !unrun.is_empty() {
        CheckResult {
            name: "revert_effective".into(),
            passed: false,
            review: true,
            detail: format!(
                "could not show the behaviour is gone; these requirements could not be run even with the reverted change's tests restored: {}",
                unrun.join(", ")
            ),
        }
    } else {
        CheckResult {
            name: "revert_effective".into(),
            passed: true,
            detail: format!("{} reverted requirement(s) no longer hold", retiring.len()),
            ..Default::default()
        }
    })
}

fn commit_step(
    s: &Session,
    op: &mut Operation,
    i: usize,
    ctx: &StepCtx,
    tree: Oid,
    by: &str,
) -> Result<()> {
    let tip = Oid::from_str(&op.tip)?;
    let step = op.steps[i].clone();
    let validation = step.attempts.last().map(|a| a.validation.clone());
    let tip_tree = s.repo.git.find_commit(tip)?.tree()?;
    if tip_tree.id() == tree && !matches!(step.kind, StepKind::Merge { .. }) {
        // Nothing to commit: the step was already satisfied.
        if let Some(v) = &validation {
            op.warnings.extend(v.warnings.iter().cloned());
        }
        op.steps[i].status = StepStatus::Done;
        op.steps[i].result_commit = Some(op.tip.clone());
        return Ok(());
    }
    let new_tree = s.repo.git.find_tree(tree)?;
    let d = s.repo.diff_trees(Some(&tip_tree), &new_tree)?;
    let mut parents = vec![tip];
    let tests_ran = validation
        .as_ref()
        .map(|v| v.checks.iter().any(|c| c.name == "tests" && c.passed))
        .unwrap_or(false);
    let verified = validation
        .as_ref()
        .map(|v| v.checks.iter().any(|c| c.name == "verifier" && c.passed))
        .unwrap_or(false);
    let mut confidence: f64 = 0.45
        + if tests_ran { 0.3 } else { 0.0 }
        + if verified { 0.15 } else { 0.0 }
        + if by == "git" { 0.05 } else { 0.0 };
    confidence = confidence.min(0.95);

    let symbols = diff::symbols_from_patch(&d.patch);
    let files: Vec<String> = d.files.iter().map(|f| f.path.clone()).collect();
    let mut rec;
    let message = match &step.kind {
        StepKind::Apply { source } => {
            let (_, src) = &ctx.records[0];
            rec = src.clone();
            rec.provenance.derived_from = Some(SourceRef {
                commit: source.clone(),
                intent_id: src.intent_id.clone(),
            });
            // Same requirement ids, re-bound to this realisation (by test name).
            for r in rec.requirements.iter_mut() {
                r.rebound_from = Some(source.clone());
                r.proof = None;
            }
            rec.retires = vec![];
            let src_msg = s
                .repo
                .git
                .find_commit(Oid::from_str(source)?)?
                .message()
                .unwrap_or("")
                .to_string();
            let first = src_msg.lines().next().unwrap_or(&src.title).to_string();
            format!(
                "{first}\n\nRe-implemented by gitent {} from {} ({}).\nIntent: {}\n\nGitent-Intent: {}\nGitent-Derived-From: {}\n",
                op.kind,
                short(source),
                if by == "git" { "deterministic" } else { "agent" },
                src.intent,
                src.intent_id,
                source
            )
        }
        StepKind::Revert { source } => {
            let (_, src) = &ctx.records[0];
            rec = src.clone();
            rec.intent_id = crate::analyze::make_intent_id(&["revert", &src.intent_id, source]);
            rec.kind = ChangeKind::Revert;
            rec.title = format!("Revert \"{}\"", src.title);
            rec.intent = format!(
                "Remove the behaviour introduced by {} ({}).",
                src.intent_id, src.title
            );
            rec.behavior = format!(
                "The behaviour described as \"{}\" no longer applies.",
                src.behavior
            );
            rec.retires = src
                .requirements
                .iter()
                .map(|r| Retirement {
                    id: r.id.clone(),
                    reason: format!("reverted \"{}\"", src.title),
                })
                .collect();
            rec.requirements = vec![];
            rec.provenance.reverts = Some(SourceRef {
                commit: source.clone(),
                intent_id: src.intent_id.clone(),
            });
            format!(
                "{}\n\nReverted by gitent ({}) from {}.\n\nGitent-Intent: {}\nGitent-Reverts: {}\n",
                rec.title,
                if by == "git" {
                    "deterministic"
                } else {
                    "agent"
                },
                short(source),
                rec.intent_id,
                src.intent_id
            )
        }
        StepKind::Merge {
            theirs,
            theirs_name,
            ..
        } => {
            parents.push(Oid::from_str(theirs)?);
            let branch = op.branch_ref.trim_start_matches("refs/heads/");
            let titles: Vec<String> = ctx.records.iter().map(|(_, r)| r.title.clone()).collect();
            rec = SemanticChange {
                schema: SCHEMA.into(),
                intent_id: crate::analyze::make_intent_id(&["merge", theirs, &op.tip]),
                kind: ChangeKind::Merge,
                title: format!("Merge {theirs_name} into {branch}"),
                intent: format!(
                    "Compose the intentions of {theirs_name} with {branch}: {}",
                    titles.join("; ")
                ),
                behavior: "All composed intentions hold simultaneously; the ledgers of both parents remain live.".into(),
                ..Default::default()
            };
            rec.provenance.composes = ctx
                .records
                .iter()
                .chain(ctx.preserve.iter())
                .map(|(o, r)| SourceRef {
                    commit: o.to_string(),
                    intent_id: r.intent_id.clone(),
                })
                .collect();
            rec.provenance.analysis = AnalysisInfo {
                method: "composed".into(),
                ..Default::default()
            };
            format!(
                "{}\n\nMerged by gitent ({}).\nComposes: {}\n\nGitent-Intent: {}\n",
                rec.title,
                if by == "git" {
                    "deterministic"
                } else {
                    "agent"
                },
                ctx.records
                    .iter()
                    .map(|(_, r)| r.intent_id.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
                rec.intent_id
            )
        }
    };
    // Deterministic facts of *this* realisation.
    let mut syms = symbols;
    for s0 in &rec.affected.symbols {
        if !syms.iter().any(|x| x.name == s0.name) && d.patch.contains(&s0.name) {
            syms.push(s0.clone());
        }
    }
    rec.affected = Affected {
        files,
        symbols: syms,
    };
    rec.tests = diff::tests_added_in_patch(&d.patch);
    rec.confidence = confidence;
    rec.provenance.operation = op.kind.clone();
    rec.provenance.executor = by.to_string();
    rec.provenance.attempts = step.attempts.len() as u32;
    rec.provenance.op_id = op.id.clone();
    rec.provenance.validation = validation.clone();
    rec.provenance.created_at =
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    if let Some(v) = &validation {
        op.warnings.extend(v.warnings.iter().cloned());
    }

    let commit = s.repo.commit(None, &message, tree, &parents)?;
    // Prove ported requirements still discriminate on the target (spec §7).
    if matches!(step.kind, StepKind::Apply { .. }) && s.cfg.requirements.prove != "off" {
        let rep = requirements::prove(
            &s.repo,
            &s.cfg,
            &mut rec.requirements,
            commit,
            Some(tip),
            true,
        )?;
        for id in rep.non_discriminating {
            op.warnings.push(format!(
                "non_discriminating:{id} passes on the target even without the change"
            ));
        }
    }
    rec.realization = store::realization(&s.repo, commit)?;
    store::put(&s.repo, commit, &rec)?;
    let wt = Repo::open_exact(Path::new(&op.worktree))?;
    Repo::reset_worktree(&wt, commit, None)?;
    op.tip = commit.to_string();
    op.steps[i].status = StepStatus::Done;
    op.steps[i].result_commit = Some(commit.to_string());
    Ok(())
}

const COMPAT_SYSTEM: &str = r#"You are Gitent's merge analyst. Two branches diverged. Decide whether their INTENTIONS can
hold at the same time in one code base (textual conflicts do not matter; behavioural contradictions do, e.g. one side
requires X to be cached and the other requires X to always hit the database). Respond with ONE JSON object only:
{"compatible": true|false, "confidence": 0.0-1.0, "conflicts": [{"ours": "intent id", "theirs": "intent id", "reason": "..."}]}"#;

fn compatibility(s: &Session, ctx: &StepCtx) -> Result<Option<serde_json::Value>> {
    let Some(p) = s.provider() else {
        return Ok(None);
    };
    let fmt = |recs: &[(Oid, SemanticChange)]| -> String {
        recs.iter()
            .map(|(_, r)| {
                format!(
                    "- {} `{}`\n  why: {}\n  behaviour: {}\n  constraints: {}\n  requirements: {}",
                    r.title,
                    r.intent_id,
                    r.intent,
                    r.behavior,
                    r.statements("constraint").join(" | "),
                    r.statements("behavior").join(" | ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let prompt = format!(
        "## Ours\n{}\n\n## Theirs\n{}",
        fmt(&ctx.preserve),
        fmt(&ctx.records)
    );
    match p
        .complete(COMPAT_SYSTEM, &[Message::user(prompt)])
        .and_then(|t| extract_json(&t))
    {
        Ok(v) => Ok(Some(v)),
        Err(e) => Ok(Some(
            serde_json::json!({"compatible": null, "error": format!("{e:#}")}),
        )),
    }
}
