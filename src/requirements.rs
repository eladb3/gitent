//! Requirements: collection, execution, proof, ledger (ADR 0008, docs/REQUIREMENTS.md).

use crate::config::Config;
use crate::diff;
use crate::llm::{extract_json, Message, Provider};
use crate::model::*;
use crate::repo::{DiffInfo, Repo};
use crate::store;
use anyhow::Result;
use git2::Oid;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------- collection

/// Build the requirement list for a new record (spec §3).
pub fn collect(
    intent_id: &str,
    d: &DiffInfo,
    symbols: &[Symbol],
    author: &[RequirementDraft],
    draft: &IntentDraft,
    draft_origin: &str,
    warnings: &mut Vec<String>,
) -> Vec<Requirement> {
    let added = diff::tests_added_in_patch(&d.patch);
    let mut touched: BTreeSet<String> = added.iter().cloned().collect();
    for s in symbols {
        if s.name.starts_with("test") || s.name.starts_with("Test") {
            touched.insert(s.name.clone());
        }
    }
    let mut out: Vec<Requirement> = vec![];
    let mut n = 0usize;
    let mut next_id = |key: &str| {
        n += 1;
        format!(
            "req_{}",
            crate::analyze::short_hash(&[intent_id, key, &n.to_string()])
        )
    };

    // 1. auto: every added test.
    for t in &added {
        out.push(Requirement {
            id: next_id(t),
            statement: humanize_test(t),
            category: "behavior".into(),
            kind: ReqKind::Test,
            test: t.clone(),
            origin: "auto".into(),
            ..Default::default()
        });
    }

    // 2. author flags, then draft requirements, then draft acceptance/constraints.
    let mut drafts: Vec<(RequirementDraft, &str)> =
        author.iter().cloned().map(|r| (r, "author")).collect();
    drafts.extend(
        draft
            .requirements
            .iter()
            .cloned()
            .map(|r| (r, draft_origin)),
    );
    for a in &draft.acceptance {
        drafts.push((
            RequirementDraft {
                statement: a.clone(),
                category: "behavior".into(),
                ..Default::default()
            },
            draft_origin,
        ));
    }
    for c in &draft.constraints {
        drafts.push((
            RequirementDraft {
                statement: c.clone(),
                category: "constraint".into(),
                ..Default::default()
            },
            draft_origin,
        ));
    }

    for (r, origin) in drafts {
        let statement = r.statement.trim().to_string();
        let category = if r.category == "constraint" {
            "constraint"
        } else {
            "behavior"
        }
        .to_string();
        let mut kind = match r.kind.as_str() {
            "test" => ReqKind::Test,
            "command" => ReqKind::Command,
            "judged" => ReqKind::Judged,
            _ if r.check.is_some() => ReqKind::Command,
            _ if !r.test.trim().is_empty() => ReqKind::Test,
            _ => ReqKind::Judged,
        };
        let test = r.test.trim().to_string();
        let mut check = r.check.clone();
        if origin == "llm" {
            if kind == ReqKind::Command {
                warnings.push(format!(
                    "LLM-proposed command check for \"{statement}\" not accepted; stored as judged"
                ));
                kind = ReqKind::Judged;
                check = None;
            } else if kind == ReqKind::Test && !touched.contains(&test) {
                warnings.push(format!("LLM linked \"{statement}\" to test `{test}` which is not in the diff; stored as judged"));
                kind = ReqKind::Judged;
            }
        }
        if kind == ReqKind::Command
            && check
                .as_ref()
                .map(|c| c.run.trim().is_empty())
                .unwrap_or(true)
        {
            warnings.push(format!(
                "requirement \"{statement}\" has kind command but no check; stored as judged"
            ));
            kind = ReqKind::Judged;
            check = None;
        }
        if kind == ReqKind::Test && test.is_empty() {
            kind = ReqKind::Judged;
        }
        // A draft naming an auto test refines that requirement's statement.
        if kind == ReqKind::Test {
            if let Some(existing) = out
                .iter_mut()
                .find(|x| x.kind == ReqKind::Test && x.test == test)
            {
                if !statement.is_empty() {
                    existing.statement = statement;
                    existing.category = category;
                    existing.origin = origin.into();
                }
                continue;
            }
        }
        if statement.is_empty() && test.is_empty() {
            continue;
        }
        if out
            .iter()
            .any(|x| !statement.is_empty() && x.statement == statement && x.kind == kind)
        {
            continue;
        }
        out.push(Requirement {
            id: next_id(if statement.is_empty() {
                &test
            } else {
                &statement
            }),
            statement: if statement.is_empty() {
                humanize_test(&test)
            } else {
                statement
            },
            category,
            kind,
            test: if kind == ReqKind::Test {
                test
            } else {
                String::new()
            },
            check: if kind == ReqKind::Command {
                check
            } else {
                None
            },
            origin: origin.into(),
            ..Default::default()
        });
    }
    out
}

// ---------------------------------------------------------------- execution

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ReqResult {
    pub id: String,
    pub statement: String,
    pub kind: ReqKind,
    /// pass | fail | skipped | uncertain
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intent_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intent_title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub introduced_by: String,
    pub duration_ms: u128,
}

/// Runs requirement checks in one directory (memoizing test discovery and the full suite).
pub struct Runner<'a> {
    pub dir: PathBuf,
    pub cfg: &'a Config,
    tests: Option<BTreeSet<String>>,
    suite: Option<(bool, String)>,
}

impl<'a> Runner<'a> {
    pub fn new(dir: &Path, cfg: &'a Config) -> Self {
        Runner {
            dir: dir.to_path_buf(),
            cfg,
            tests: None,
            suite: None,
        }
    }

    fn tests_present(&mut self) -> &BTreeSet<String> {
        if self.tests.is_none() {
            self.tests = Some(diff::tests_in_tree(&self.dir));
        }
        self.tests.as_ref().unwrap()
    }

    fn sh(&self, cmd: &str, timeout: u64) -> (bool, String) {
        match crate::proc::run_shell(cmd, &self.dir, None, &[], Duration::from_secs(timeout)) {
            Ok(o) => (
                o.success(),
                if o.success() {
                    String::new()
                } else {
                    o.combined_tail(1500)
                },
            ),
            Err(e) => (false, format!("{e:#}")),
        }
    }

    /// Execute one executable requirement. Judged requirements return `skipped`.
    pub fn run(&mut self, r: &Requirement) -> (String, String, u128) {
        let start = std::time::Instant::now();
        let (status, detail) = match r.kind {
            ReqKind::Judged => (
                "skipped".to_string(),
                "judged requirement (not executable)".to_string(),
            ),
            ReqKind::Test => {
                if !self.tests_present().contains(&r.test) {
                    ("fail".into(), format!("test `{}` does not exist", r.test))
                } else if !self.cfg.validation.test_selector.trim().is_empty() {
                    let cmd = self.cfg.validation.test_selector.replace("{test}", &r.test);
                    let (ok, out) = self.sh(&cmd, self.cfg.validation.timeout_secs);
                    (
                        if ok { "pass" } else { "fail" }.into(),
                        if ok {
                            String::new()
                        } else {
                            format!("`{cmd}` failed\n{out}")
                        },
                    )
                } else if !self.cfg.validation.test_command.trim().is_empty() {
                    if self.suite.is_none() {
                        let cmd = self.cfg.validation.test_command.clone();
                        self.suite = Some(self.sh(&cmd, self.cfg.validation.timeout_secs));
                    }
                    let (ok, out) = self.suite.clone().unwrap();
                    (
                        if ok { "pass" } else { "fail" }.into(),
                        if ok {
                            "no test_selector: test exists and the full suite passes".into()
                        } else {
                            format!("no test_selector: full suite failed\n{out}")
                        },
                    )
                } else {
                    (
                        "skipped".into(),
                        "no test_selector or test_command configured".into(),
                    )
                }
            }
            ReqKind::Command => match &r.check {
                Some(c) => match crate::proc::run_shell(
                    &c.run,
                    &self.dir,
                    None,
                    &[],
                    Duration::from_secs(c.timeout_secs),
                ) {
                    Ok(o) => {
                        let ok = !o.timed_out && o.code == Some(c.expect_exit);
                        (
                            if ok { "pass" } else { "fail" }.into(),
                            if ok {
                                String::new()
                            } else {
                                format!(
                                    "`{}` exit {:?} (expected {})\n{}",
                                    c.run,
                                    o.code,
                                    c.expect_exit,
                                    o.combined_tail(1500)
                                )
                            },
                        )
                    }
                    Err(e) => ("fail".into(), format!("{e:#}")),
                },
                None => ("fail".into(), "command requirement without check".into()),
            },
        };
        (status, detail, start.elapsed().as_millis())
    }

    pub fn run_all(&mut self, reqs: &[LiveReq]) -> Vec<ReqResult> {
        reqs.iter()
            .map(|l| {
                let (status, detail, ms) = self.run(&l.req);
                ReqResult {
                    id: l.req.id.clone(),
                    statement: l.req.statement.clone(),
                    kind: l.req.kind,
                    status,
                    detail,
                    intent_id: l.intent_id.clone(),
                    intent_title: l.title.clone(),
                    introduced_by: l.commit.clone(),
                    duration_ms: ms,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------- ledger

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LiveReq {
    pub req: Requirement,
    pub intent_id: String,
    pub title: String,
    /// Most recent commit realising the requirement in this history.
    pub commit: String,
}

/// Live requirements at `tip`: introduced in its ancestry, minus retired (spec §5). Oldest first.
pub fn ledger(repo: &Repo, tip: Oid) -> Result<Vec<LiveReq>> {
    let mut walk = repo.git.revwalk()?;
    walk.push(tip)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut retired: HashSet<String> = HashSet::new();
    let mut out = vec![];
    for oid in walk {
        let oid = oid?;
        let Some(rec) = store::get(repo, oid) else {
            continue;
        };
        for r in &rec.retires {
            retired.insert(r.id.clone());
        }
        for req in &rec.requirements {
            if seen.insert(req.id.clone()) {
                out.push(LiveReq {
                    req: req.clone(),
                    intent_id: rec.intent_id.clone(),
                    title: rec.title.clone(),
                    commit: oid.to_string(),
                });
            }
        }
    }
    out.retain(|l| !retired.contains(&l.req.id));
    out.reverse();
    Ok(out)
}

/// Union of ledgers (merge), de-duplicated by id.
pub fn union(a: Vec<LiveReq>, b: Vec<LiveReq>) -> Vec<LiveReq> {
    let mut ids: HashSet<String> = a.iter().map(|l| l.req.id.clone()).collect();
    let mut out = a;
    for l in b {
        if ids.insert(l.req.id.clone()) {
            out.push(l);
        }
    }
    out
}

pub fn live_from_record(rec: &SemanticChange, commit: &str) -> Vec<LiveReq> {
    rec.requirements
        .iter()
        .map(|r| LiveReq {
            req: r.clone(),
            intent_id: rec.intent_id.clone(),
            title: rec.title.clone(),
            commit: commit.to_string(),
        })
        .collect()
}

// ---------------------------------------------------------------- proof

pub fn is_test_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let segs: Vec<&str> = p.split('/').collect();
    let name = segs.last().copied().unwrap_or("");
    segs[..segs.len().saturating_sub(1)]
        .iter()
        .any(|s| matches!(*s, "test" | "tests" | "spec" | "__tests__"))
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains(".test.")
        || name.contains(".spec.")
}

#[derive(Debug, Default)]
pub struct ProofReport {
    pub after_failures: Vec<(String, String)>,
    pub non_discriminating: Vec<String>,
}

/// A temporary worktree under `.git/gitent/tmp/` removed on drop.
pub struct TempTree<'r> {
    repo: &'r Repo,
    pub path: PathBuf,
}

impl<'r> TempTree<'r> {
    pub fn new(repo: &'r Repo, commit: Oid, label: &str) -> Result<Self> {
        let path = repo
            .gitent_dir()
            .join("tmp")
            .join(format!("{label}-{}", crate::ops::new_op_id()));
        repo.add_worktree(&path, commit)?;
        Ok(TempTree { repo, path })
    }
}

impl Drop for TempTree<'_> {
    fn drop(&mut self) {
        let _ = self.repo.remove_worktree(&self.path);
    }
}

/// Write the test files of `new` (relative to `parent`) into `dir`.
pub fn overlay_test_files(repo: &Repo, parent: Oid, new: Oid, dir: &Path) -> Result<Vec<String>> {
    let pt = repo.git.find_commit(parent)?.tree()?;
    let nt = repo.git.find_commit(new)?.tree()?;
    let d = repo.diff_trees(Some(&pt), &nt)?;
    let added_tests = diff::tests_added_in_patch(&d.patch);
    let mut written = vec![];
    for f in &d.files {
        if f.status == "deleted" {
            continue;
        }
        let entry = match nt.get_path(Path::new(&f.path)) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let blob = match repo.git.find_blob(entry.id()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let content = String::from_utf8_lossy(blob.content());
        let defines_new_test = content.lines().any(|l| {
            diff::test_in_line(l)
                .map(|t| added_tests.contains(&t))
                .unwrap_or(false)
        });
        if is_test_file(&f.path) || defines_new_test {
            let target = dir.join(&f.path);
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&target, blob.content())?;
            written.push(f.path.clone());
        }
    }
    Ok(written)
}

/// Prove executable requirements of a new commit (spec §4): fail on `parent`
/// (with the commit's test files overlaid), pass on `new`. Updates `proof`.
pub fn prove(
    repo: &Repo,
    cfg: &Config,
    reqs: &mut [Requirement],
    new: Oid,
    parent: Option<Oid>,
    after_known: bool,
) -> Result<ProofReport> {
    let mut rep = ProofReport::default();
    if !reqs.iter().any(|r| r.kind.executable()) {
        return Ok(rep);
    }
    let mut after: HashMap<String, String> = HashMap::new();
    if after_known {
        for r in reqs.iter() {
            after.insert(r.id.clone(), "pass".into());
        }
    } else {
        let t = TempTree::new(repo, new, "after")?;
        let mut run = Runner::new(&t.path, cfg);
        for r in reqs.iter().filter(|r| r.kind.executable()) {
            let (st, detail, _) = run.run(r);
            if st == "fail" {
                rep.after_failures.push((r.id.clone(), detail));
            }
            after.insert(r.id.clone(), st);
        }
    }
    let mut before: HashMap<String, String> = HashMap::new();
    if let Some(p) = parent {
        let t = TempTree::new(repo, p, "before")?;
        overlay_test_files(repo, p, new, &t.path)?;
        let mut run = Runner::new(&t.path, cfg);
        for r in reqs.iter().filter(|r| r.kind.executable()) {
            let (st, _, _) = run.run(r);
            if st == "pass" {
                rep.non_discriminating.push(r.id.clone());
            }
            before.insert(r.id.clone(), st);
        }
    }
    for r in reqs.iter_mut().filter(|r| r.kind.executable()) {
        r.proof = Some(Proof {
            before: before
                .get(&r.id)
                .cloned()
                .unwrap_or_else(|| "skipped".into()),
            after: after
                .get(&r.id)
                .cloned()
                .unwrap_or_else(|| "skipped".into()),
            commit: new.to_string(),
        });
    }
    Ok(rep)
}

/// Test name → body text, by indentation (good enough for Python and brace languages
/// with conventional formatting).
pub fn test_bodies(text: &str) -> HashMap<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = HashMap::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(name) = diff::test_in_line(l) {
            let base = indent(l);
            let mut body = vec![*l];
            for m in &lines[i + 1..] {
                if !m.trim().is_empty()
                    && indent(m) <= base
                    && !m.trim_start().starts_with('}')
                    && !m.trim_start().starts_with(')')
                {
                    break;
                }
                body.push(m);
            }
            out.insert(name, body.join("\n").trim_end().to_string());
        }
    }
    out
}

/// Tests whose body differs between two trees (only files in `paths` are compared).
pub fn modified_tests(
    repo: &Repo,
    old: Option<&git2::Tree>,
    new: &git2::Tree,
    paths: &[String],
) -> HashSet<String> {
    let read = |t: &git2::Tree, p: &str| -> String {
        t.get_path(Path::new(p))
            .ok()
            .and_then(|e| repo.git.find_blob(e.id()).ok())
            .map(|b| String::from_utf8_lossy(b.content()).to_string())
            .unwrap_or_default()
    };
    let mut out = HashSet::new();
    for p in paths {
        let before = old.map(|t| test_bodies(&read(t, p))).unwrap_or_default();
        let after = test_bodies(&read(new, p));
        for (name, body) in &before {
            if after.get(name) != Some(body) {
                out.insert(name.clone());
            }
        }
    }
    out
}

// ---------------------------------------------------------------- judging

const JUDGE_SYSTEM: &str = r#"You are Gitent's requirement judge. Decide whether the code satisfies the requirement.
Respond with ONE JSON object only: {"satisfied": true|false, "confidence": 0.0-1.0, "reason": "..."}"#;

/// Judge a requirement against the content of the given files (best effort).
pub fn judge(
    p: &dyn Provider,
    statement: &str,
    dir: &Path,
    files: &[String],
    threshold: f64,
) -> (String, String) {
    let mut ctx = String::new();
    for f in files {
        if let Ok(t) = std::fs::read_to_string(dir.join(f)) {
            ctx.push_str(&format!(
                "### {f}\n```\n{}\n```\n",
                crate::proc::tail(&t, 8000)
            ));
        }
        if ctx.len() > 30_000 {
            break;
        }
    }
    let prompt = format!("## Requirement\n{statement}\n\n## Code\n{ctx}");
    match p
        .complete(JUDGE_SYSTEM, &[Message::user(prompt)])
        .and_then(|t| extract_json(&t))
    {
        Ok(v) => {
            let sat = v["satisfied"].as_bool().unwrap_or(false);
            let conf = v["confidence"].as_f64().unwrap_or(0.5);
            let reason = v["reason"].as_str().unwrap_or("").to_string();
            let st = if sat {
                "pass"
            } else if conf >= threshold {
                "fail"
            } else {
                "uncertain"
            };
            (
                st.into(),
                format!("judged (confidence {conf:.2}): {reason}"),
            )
        }
        Err(e) => ("uncertain".into(), format!("judge unavailable: {e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::FileChange;

    fn diff_with_test() -> DiffInfo {
        DiffInfo {
            patch: "--- a/t/test_u.py\n+++ b/t/test_u.py\n@@ -0,0 +1,2 @@\n+def test_cache_hit():\n+    pass\n".into(),
            files: vec![FileChange { path: "t/test_u.py".into(), old_path: None, status: "added".into() }],
        }
    }

    #[test]
    fn collects_auto_author_and_checks_llm() {
        let d = diff_with_test();
        let mut w = vec![];
        let draft = IntentDraft {
            requirements: vec![
                RequirementDraft {
                    statement: "Second lookup is cached".into(),
                    test: "test_cache_hit".into(),
                    ..Default::default()
                },
                RequirementDraft {
                    statement: "ghost".into(),
                    test: "test_ghost".into(),
                    ..Default::default()
                },
                RequirementDraft {
                    statement: "fast".into(),
                    check: Some(CheckSpec {
                        run: "rm -rf /".into(),
                        expect_exit: 0,
                        timeout_secs: 1,
                    }),
                    ..Default::default()
                },
            ],
            constraints: vec!["signature unchanged".into()],
            ..Default::default()
        };
        let author = vec![RequirementDraft {
            statement: "lint passes".into(),
            check: Some(CheckSpec {
                run: "true".into(),
                expect_exit: 0,
                timeout_secs: 5,
            }),
            ..Default::default()
        }];
        let r = collect("int_x", &d, &[], &author, &draft, "llm", &mut w);
        let by = |s: &str| {
            r.iter()
                .find(|x| x.statement == s)
                .unwrap_or_else(|| panic!("{s} in {r:?}"))
                .clone()
        };
        let auto = by("Second lookup is cached");
        assert_eq!(
            (auto.kind, auto.test.as_str(), auto.origin.as_str()),
            (ReqKind::Test, "test_cache_hit", "llm")
        );
        assert_eq!(by("lint passes").kind, ReqKind::Command);
        assert_eq!(by("ghost").kind, ReqKind::Judged);
        let fast = by("fast");
        assert_eq!(fast.kind, ReqKind::Judged);
        assert!(fast.check.is_none(), "LLM commands are never kept");
        assert_eq!(by("signature unchanged").category, "constraint");
        assert_eq!(w.len(), 2, "{w:?}");
        let ids: HashSet<_> = r.iter().map(|x| x.id.clone()).collect();
        assert_eq!(ids.len(), r.len());
        assert!(r
            .iter()
            .all(|x| x.id.starts_with("req_") && x.id.len() == 12));
    }

    #[test]
    fn detects_test_body_changes() {
        let a = "class C:\n    def test_a(self):\n        x = 1\n        assert x == 1\n\n    def test_b(self):\n        pass\n";
        let b = a.replace("assert x == 1", "assert x == 2");
        let (ba, bb) = (test_bodies(a), test_bodies(&b));
        assert_ne!(ba["test_a"], bb["test_a"]);
        assert_eq!(ba["test_b"], bb["test_b"]);
        let rs = "#[test]\nfn test_x() {\n    assert!(true);\n}\n\nfn other() {}\n";
        assert!(test_bodies(rs)["test_x"].contains("assert!(true)"));
        assert!(!test_bodies(rs)["test_x"].contains("other"));
    }

    #[test]
    fn test_file_heuristic() {
        assert!(is_test_file("tests/test_a.py"));
        assert!(is_test_file("pkg/foo_test.go"));
        assert!(is_test_file("src/a.test.ts"));
        assert!(!is_test_file("src/app.py"));
        assert!(!is_test_file("contest.py"));
    }
}
