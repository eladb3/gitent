//! `gitent bench`: benchmark coding agents on your own history (ADR 0018).
//!
//! `mine` turns commits whose tests fail before them and pass after them into
//! tasks; `run` gives each task to an agent in an isolated repository (no
//! history, so the answer cannot be looked up) and scores it on the commit's
//! hidden tests; `compare` puts runs side by side.

use super::prove_pr::{prove_tests, shape};
use super::Session;
use crate::agent::{self, ExecutorChoice};
use crate::config::Config;
use crate::model::{ReqKind, Requirement};
use crate::repo::short;
use crate::requirements::{overlay_test_files, Runner};
use anyhow::{bail, Context, Result};
use git2::{Oid, Repository};
use regex::Regex;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

pub const SCHEMA: &str = "gitent/bench@1";
pub const RUN_SCHEMA: &str = "gitent/bench-run@1";

#[derive(Debug, Clone, Default)]
pub struct MineOptions {
    /// A revision range (`A..B`) or a revision (its last `limit_commits` first-parent commits).
    pub range: Option<String>,
    /// Stop after this many tasks.
    pub limit: Option<usize>,
    /// How many commits to look at when no range is given.
    pub scan: usize,
    pub max_files: usize,
}

/// A commit message as a problem statement: trailers and cherry-pick lines removed.
pub fn statement(message: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^(?:signed-off-by|co-authored-by|gitent-[\w-]+|change-id|reviewed-by|reviewed-on|acked-by|tested-by|claude-session):|^\(cherry picked from commit ").unwrap()
    });
    let kept: Vec<&str> = message.lines().filter(|l| !re.is_match(l.trim())).collect();
    kept.join("\n").trim().to_string()
}

fn first_parent_commits(repo: &crate::repo::Repo, o: &MineOptions) -> Result<Vec<Oid>> {
    let mut walk = repo.git.revwalk()?;
    walk.simplify_first_parent()?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL)?;
    let mut cap = None;
    match o.range.as_deref() {
        Some(r) if r.contains("..") => walk.push_range(r)?,
        Some(r) => {
            walk.push(repo.resolve(r)?)?;
            cap = Some(o.scan);
        }
        None => {
            walk.push_head()?;
            cap = Some(o.scan);
        }
    }
    let mut out = vec![];
    for oid in walk {
        out.push(oid?);
        if cap.map(|c| out.len() >= c).unwrap_or(false) {
            break;
        }
    }
    Ok(out)
}

pub fn mine(s: &Session, o: &MineOptions) -> Result<Value> {
    let repo = &s.repo;
    if s.cfg.validation.test_selector.trim().is_empty()
        && s.cfg.validation.test_command.trim().is_empty()
    {
        bail!("bench mine needs validation.test_command (and ideally test_selector) to prove tests; run `gitent init` or pass --test-command/--test-selector");
    }
    let commits = first_parent_commits(repo, o)?;
    let mut tasks = vec![];
    let mut scanned = 0;
    let mut skipped = serde_json::Map::new();
    let mut skip = |why: &str| {
        let n = skipped.get(why).and_then(|v| v.as_u64()).unwrap_or(0);
        skipped.insert(why.to_string(), json!(n + 1));
    };
    for (i, c) in commits.iter().enumerate() {
        if o.limit.map(|l| tasks.len() >= l).unwrap_or(false) {
            break;
        }
        scanned += 1;
        let commit = repo.git.find_commit(*c)?;
        if commit.parent_count() != 1 {
            skip("merge or root commit");
            continue;
        }
        let parent = commit.parent_id(0)?;
        let sh = shape(repo, parent, *c)?;
        if sh.code_files.is_empty() {
            skip("no code change");
            continue;
        }
        if sh.tests.is_empty() {
            skip("no test added or modified");
            continue;
        }
        let d = repo.diff_trees(
            Some(&repo.git.find_commit(parent)?.tree()?),
            &commit.tree()?,
        )?;
        if d.files.len() > o.max_files {
            skip("too many files");
            continue;
        }
        eprintln!(
            "gitent bench: [{}/{}] {} proving {} test(s)…",
            i + 1,
            commits.len(),
            short(&c.to_string()),
            sh.tests.len()
        );
        let rows = prove_tests(s, parent, *c, &sh.tests)?;
        let hidden: Vec<Value> = rows
            .iter()
            .filter(|r| r["status"] == "proven" || r["status"] == "proven_api")
            .map(|r| {
                json!({
                    "test": r["test"], "file": r["file"],
                    "proof": if r["status"] == "proven" { "assertion" } else { "missing_api" },
                })
            })
            .collect();
        if hidden.is_empty() {
            skip("no test fails without the commit");
            continue;
        }
        // Lines changed in code files only (tests excluded): a difficulty proxy.
        let mut code_lines = 0;
        let mut in_code = false;
        for l in d.patch.lines() {
            if let Some(p) = l.strip_prefix("+++ b/") {
                in_code = sh.code_files.iter().any(|c| c == p.trim());
            } else if l.starts_with("+++") || l.starts_with("---") {
                continue;
            } else if in_code && (l.starts_with('+') || l.starts_with('-')) {
                code_lines += 1;
            }
        }
        let text = statement(commit.message().unwrap_or(""));
        tasks.push(json!({
            "id": short(&c.to_string()),
            "commit": c.to_string(),
            "parent": parent.to_string(),
            "title": text.lines().next().unwrap_or(""),
            "statement": text,
            "hidden_tests": hidden,
            "size": {"files": d.files.len(), "code_files": sh.code_files.len(), "lines": code_lines},
            "date": chrono::DateTime::from_timestamp(commit.time().seconds(), 0).map(|t| t.to_rfc3339()).unwrap_or_default(),
        }));
    }
    Ok(json!({
        "schema": SCHEMA,
        "repo": repo.workdir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "created_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "test_command": s.cfg.validation.test_command,
        "test_selector": s.cfg.validation.test_selector,
        "scanned": scanned,
        "skipped": skipped,
        "tasks": tasks,
    }))
}

// ---------------------------------------------------------------- run

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub bench: PathBuf,
    /// An executor preset (`claude-code`, `codex`, …); default: the configured executor.
    pub agent: Option<String>,
    pub tasks: Vec<String>,
    /// Keep each task's workspace for inspection.
    pub keep: bool,
}

/// Write `tree` into `dir` (files, modes, symlinks; submodules skipped).
fn write_tree(repo: &Repository, tree: &git2::Tree, dir: &Path) -> Result<()> {
    let mut err: Option<anyhow::Error> = None;
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() != Some(git2::ObjectType::Blob) {
            return git2::TreeWalkResult::Ok;
        }
        let rel = format!("{root}{}", entry.name().unwrap_or(""));
        let res = (|| -> Result<()> {
            let path = dir.join(&rel);
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let blob = repo.find_blob(entry.id())?;
            if entry.filemode() == 0o120000 {
                #[cfg(unix)]
                std::os::unix::fs::symlink(
                    String::from_utf8_lossy(blob.content()).to_string(),
                    &path,
                )?;
                return Ok(());
            }
            std::fs::write(&path, blob.content())?;
            #[cfg(unix)]
            if entry.filemode() == 0o100755 {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
            }
            Ok(())
        })();
        if let Err(e) = res {
            err = Some(e);
            return git2::TreeWalkResult::Abort;
        }
        git2::TreeWalkResult::Ok
    })?;
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// A fresh repository holding only `commit`'s files, as one commit: no history, no objects
/// from the real repository, so the answer cannot be looked up (ADR 0018).
fn isolated_workspace(src: &crate::repo::Repo, commit: Oid, dir: &Path) -> Result<()> {
    let tree = src.git.find_commit(commit)?.tree()?;
    write_tree(&src.git, &tree, dir)?;
    let r = Repository::init(dir)?;
    let mut idx = r.index()?;
    idx.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
    idx.write()?;
    let t = r.find_tree(idx.write_tree()?)?;
    let sig = git2::Signature::now("gitent bench", "bench@gitent.invalid")?;
    r.commit(Some("HEAD"), &sig, &sig, "task workspace", &t, &[])?;
    Ok(())
}

/// Lines the agent changed (tracked and untracked) relative to the workspace's commit.
fn diff_size(dir: &Path) -> (usize, usize) {
    let Ok(r) = Repository::open(dir) else {
        return (0, 0);
    };
    let Ok(head) = r.head().and_then(|h| h.peel_to_tree()) else {
        return (0, 0);
    };
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .show_untracked_content(true);
    let size = match r.diff_tree_to_workdir_with_index(Some(&head), Some(&mut opts)) {
        Ok(d) => {
            let files = d.deltas().len();
            let lines = d
                .stats()
                .map(|s| s.insertions() + s.deletions())
                .unwrap_or(0);
            (files, lines)
        }
        Err(_) => (0, 0),
    };
    size
}

fn brief(task: &Value, test_command: &str) -> String {
    format!(
        "# Task\n\nYou are working in a software repository (the current directory). Make the change \
         described below so that it works and is tested.\n\n## Problem\n\n{}\n\n## Notes\n\n\
         - The project's tests run with: `{}`\n\
         - Your change will be checked by tests you cannot see. Keep existing behaviour working.\n\
         - Edit files in place; do not commit.\n",
        task["statement"].as_str().unwrap_or(""),
        if test_command.is_empty() { "(not configured)" } else { test_command },
    )
}

pub fn run(s: &Session, o: &RunOptions) -> Result<Value> {
    let text = std::fs::read_to_string(&o.bench)
        .with_context(|| format!("reading {}", o.bench.display()))?;
    let bench: Value = serde_json::from_str(&text).context("parsing the bench file")?;
    if bench["schema"] != SCHEMA {
        bail!(
            "{} is not a gitent bench file (schema {})",
            o.bench.display(),
            SCHEMA
        );
    }
    let mut cfg: Config = s.cfg.clone();
    if cfg.validation.test_command.trim().is_empty() {
        cfg.validation.test_command = bench["test_command"].as_str().unwrap_or("").to_string();
    }
    if cfg.validation.test_selector.trim().is_empty() {
        cfg.validation.test_selector = bench["test_selector"].as_str().unwrap_or("").to_string();
    }
    let mut label = String::new();
    if let Some(a) = &o.agent {
        let p = Config::preset(a).with_context(|| {
            format!(
                "unknown agent preset '{a}' (one of: {})",
                Config::PRESETS.join(", ")
            )
        })?;
        cfg.executor = p.executor;
        if cfg.executor.kind == "builtin" {
            cfg.llm = p.llm;
        }
        label = a.clone();
    }
    let provider = if cfg.executor.kind == "builtin" {
        crate::llm::from_config(&cfg.llm, &s.repo.workdir)?
    } else {
        s.provider.clone()
    };
    let executor = match agent::from_config(&cfg, provider)? {
        ExecutorChoice::Run(e) => e,
        ExecutorChoice::Host => bail!(
            "bench run needs an agent to run: --agent <preset> ({}) or --executor command --executor-command '…'",
            Config::PRESETS.join(", ")
        ),
    };
    if label.is_empty() {
        label = executor.name();
    }
    let all = bench["tasks"].as_array().cloned().unwrap_or_default();
    let tasks: Vec<Value> = all
        .into_iter()
        .filter(|t| {
            o.tasks.is_empty()
                || o.tasks.iter().any(|id| {
                    t["id"].as_str() == Some(id.as_str())
                        || t["commit"]
                            .as_str()
                            .map(|c| c.starts_with(id.as_str()))
                            .unwrap_or(false)
                })
        })
        .collect();
    if tasks.is_empty() {
        bail!("no tasks to run");
    }
    let root = tempfile::Builder::new().prefix("gitent-bench-").tempdir()?;
    let mut results = vec![];
    for (i, t) in tasks.iter().enumerate() {
        let id = t["id"].as_str().unwrap_or("").to_string();
        let commit = Oid::from_str(t["commit"].as_str().unwrap_or(""))
            .with_context(|| format!("task {id}: bad commit"))?;
        let parent = Oid::from_str(t["parent"].as_str().unwrap_or(""))?;
        if s.repo.git.find_commit(commit).is_err() {
            bail!("task {id}: commit {commit} is not in this repository (run bench in the repository it was mined from)");
        }
        eprintln!(
            "gitent bench: [{}/{}] {id} {}",
            i + 1,
            tasks.len(),
            t["title"].as_str().unwrap_or("")
        );
        let ws = root.path().join(&id).join("repo");
        std::fs::create_dir_all(&ws)?;
        isolated_workspace(&s.repo, parent, &ws)?;
        let brief_text = brief(t, &cfg.validation.test_command);
        let brief_path = root.path().join(&id).join("TASK.md");
        std::fs::write(&brief_path, &brief_text)?;
        let start = Instant::now();
        let exec = executor.execute(&agent::Task {
            op_id: format!("bench_{id}"),
            attempt: 1,
            worktree: ws.clone(),
            brief_path: brief_path.clone(),
            brief: brief_text,
        });
        let duration_ms = start.elapsed().as_millis();
        let (claimed, summary) = match &exec {
            Ok(r) => (r.success, r.summary.clone()),
            Err(e) => (false, format!("agent error: {e:#}")),
        };
        let (diff_files, diff_lines) = diff_size(&ws);
        // Score: the commit's test files replace whatever the agent wrote there.
        overlay_test_files(&s.repo, parent, commit, &ws)?;
        let reqs: Vec<Requirement> = t["hidden_tests"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(k, h)| Requirement {
                id: format!("h{}", k + 1),
                statement: h["test"].as_str().unwrap_or("").to_string(),
                kind: ReqKind::Test,
                test: h["test"].as_str().unwrap_or("").to_string(),
                test_path: h["file"].as_str().unwrap_or("").to_string(),
                origin: "auto".into(),
                ..Default::default()
            })
            .collect();
        let mut runner = Runner::new(&ws, &cfg);
        let hidden: Vec<Value> = reqs
            .iter()
            .map(|r| {
                let (st, detail, _) = runner.run(r);
                json!({"test": r.test, "file": r.test_path, "status": st,
                       "detail": crate::proc::tail(&detail, 800)})
            })
            .collect();
        let resolved = !hidden.is_empty() && hidden.iter().all(|h| h["status"] == "pass");
        results.push(json!({
            "id": id, "commit": t["commit"], "title": t["title"],
            "resolved": resolved,
            "claimed_success": claimed,
            "false_claim": claimed && !resolved,
            "hidden_tests": hidden,
            "duration_ms": duration_ms as u64,
            "diff": {"files": diff_files, "lines": diff_lines},
            "agent_summary": crate::proc::tail(&summary, 600),
            "workspace": if o.keep { ws.to_string_lossy().to_string() } else { String::new() },
        }));
    }
    if o.keep {
        let kept = root.keep();
        eprintln!("gitent bench: workspaces kept in {}", kept.display());
    }
    let resolved = results.iter().filter(|r| r["resolved"] == true).count();
    let false_claims = results.iter().filter(|r| r["false_claim"] == true).count();
    let mut durations: Vec<u64> = results
        .iter()
        .map(|r| r["duration_ms"].as_u64().unwrap_or(0))
        .collect();
    durations.sort();
    Ok(json!({
        "schema": RUN_SCHEMA,
        "bench": o.bench.to_string_lossy(),
        "agent": label,
        "created_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "total": results.len(),
        "resolved": resolved,
        "false_claims": false_claims,
        "median_ms": durations.get(durations.len() / 2).copied().unwrap_or(0),
        "tasks": results,
    }))
}

// ---------------------------------------------------------------- compare

pub fn compare(paths: &[PathBuf]) -> Result<Value> {
    if paths.is_empty() {
        bail!("compare needs at least one results file");
    }
    let mut runs = vec![];
    for p in paths {
        let v: Value = serde_json::from_str(
            &std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
        )
        .with_context(|| format!("parsing {}", p.display()))?;
        if v["schema"] != RUN_SCHEMA {
            bail!("{} is not a bench run result ({RUN_SCHEMA})", p.display());
        }
        runs.push(v);
    }
    let mut ids: Vec<(String, String)> = vec![];
    for r in &runs {
        for t in r["tasks"].as_array().into_iter().flatten() {
            let id = t["id"].as_str().unwrap_or("").to_string();
            if !ids.iter().any(|(i, _)| i == &id) {
                ids.push((id, t["title"].as_str().unwrap_or("").to_string()));
            }
        }
    }
    let label = |r: &Value| r["agent"].as_str().unwrap_or("?").to_string();
    let cell = |r: &Value, id: &str| -> &'static str {
        match r["tasks"]
            .as_array()
            .and_then(|ts| ts.iter().find(|t| t["id"] == id))
        {
            None => "–",
            Some(t) if t["resolved"] == true => "✅",
            Some(t) if t["false_claim"] == true => "❌ claimed done",
            Some(_) => "❌",
        }
    };
    let mut md = String::from("| task |");
    for r in &runs {
        md.push_str(&format!(" {} |", label(r)));
    }
    md.push_str("\n|---|");
    md.push_str(&"---|".repeat(runs.len()));
    md.push('\n');
    for (id, title) in &ids {
        md.push_str(&format!(
            "| `{id}` {} |",
            super::verify_pr::md_inline(&title.chars().take(60).collect::<String>())
        ));
        for r in &runs {
            md.push_str(&format!(" {} |", cell(r, id)));
        }
        md.push('\n');
    }
    for (name, f) in [
        (
            "**resolved**",
            (|r: &Value| format!("{}/{}", r["resolved"], r["total"])) as fn(&Value) -> String,
        ),
        ("false claims", |r: &Value| r["false_claims"].to_string()),
        ("median time", |r: &Value| {
            format!(
                "{:.1}s",
                r["median_ms"].as_u64().unwrap_or(0) as f64 / 1000.0
            )
        }),
    ] {
        md.push_str(&format!("| {name} |"));
        for r in &runs {
            md.push_str(&format!(" {} |", f(r)));
        }
        md.push('\n');
    }
    Ok(json!({
        "runs": runs.iter().map(|r| json!({
            "agent": label(r), "resolved": r["resolved"], "total": r["total"],
            "false_claims": r["false_claims"], "median_ms": r["median_ms"],
        })).collect::<Vec<_>>(),
        "markdown": md,
    }))
}

#[cfg(test)]
mod tests {
    use super::statement;

    #[test]
    fn statements_drop_trailers() {
        let m = "fix: clamp discount\n\nDiscounts over 100% made totals negative.\n\nSigned-off-by: A <a@x>\nCo-Authored-By: B <b@x>\nGitent-Intent: int_1\n(cherry picked from commit abc1234)\n";
        assert_eq!(
            statement(m),
            "fix: clamp discount\n\nDiscounts over 100% made totals negative."
        );
    }
}
