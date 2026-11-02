//! Thin wrapper over git2 (objects, refs, notes, diffs, in-memory merges)
//! plus the git CLI for worktree management (see ADR 0001).

use anyhow::{anyhow, bail, Context, Result};
use git2::{
    build::CheckoutBuilder, Commit, DiffFindOptions, DiffFormat, DiffOptions, IndexAddOption, Oid,
    Repository, Signature, StatusOptions, Tree,
};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const NOTES_REF: &str = "refs/notes/gitent";

pub struct Repo {
    pub git: Repository,
    /// Main working directory.
    pub workdir: PathBuf,
    /// Common git dir (shared by all worktrees).
    pub git_dir: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Default)]
pub struct DiffInfo {
    pub patch: String,
    pub files: Vec<FileChange>,
}

/// Result of an in-memory merge-like operation.
pub enum MergeOutcome {
    Clean(Oid),
    /// Conflicting paths, plus a tree with every non-conflicting part applied
    /// and conflicting paths kept as on our side.
    Conflicts(Vec<String>, Option<Oid>),
}

impl Repo {
    pub fn discover(path: &Path) -> Result<Repo> {
        let git = Repository::discover(path).with_context(|| {
            format!(
                "not a git repository: {} (run `gitent init`)",
                path.display()
            )
        })?;
        Self::from_git(git)
    }

    pub fn open_exact(path: &Path) -> Result<Repo> {
        Self::from_git(Repository::open(path)?)
    }

    fn from_git(git: Repository) -> Result<Repo> {
        let workdir = git
            .workdir()
            .ok_or_else(|| anyhow!("bare repositories are not supported"))?
            .to_path_buf();
        let git_dir = git.commondir().to_path_buf();
        Ok(Repo {
            git,
            workdir,
            git_dir,
        })
    }

    pub fn init(path: &Path) -> Result<Repo> {
        std::fs::create_dir_all(path)?;
        let git = match Repository::open(path) {
            Ok(r) => r,
            Err(_) => Repository::init(path)?,
        };
        Self::from_git(git)
    }

    pub fn gitent_dir(&self) -> PathBuf {
        self.git_dir.join("gitent")
    }

    /// Root for operation/temporary worktrees: outside `.git/` (ADR 0009).
    pub fn worktree_root(&self, configured: &str) -> PathBuf {
        if !configured.trim().is_empty() {
            let p = PathBuf::from(configured);
            return if p.is_absolute() { p } else { self.workdir.join(p) };
        }
        let canon = self.git_dir.canonicalize().unwrap_or_else(|_| self.git_dir.clone());
        let id = Oid::hash_object(git2::ObjectType::Blob, canon.to_string_lossy().as_bytes())
            .map(|o| o.to_string()[..12].to_string())
            .unwrap_or_else(|_| "repo".into());
        let name = self.workdir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        std::env::temp_dir().join("gitent-worktrees").join(format!("{name}-{id}"))
    }

    pub fn signature(&self) -> Signature<'static> {
        self.git
            .signature()
            .map(|s| s.to_owned())
            .unwrap_or_else(|_| Signature::now("gitent", "gitent@localhost").unwrap())
    }

    pub fn head_commit(&self) -> Result<Option<Commit<'_>>> {
        match self.git.head() {
            Ok(h) => Ok(Some(h.peel_to_commit()?)),
            Err(e)
                if e.code() == git2::ErrorCode::UnbornBranch
                    || e.code() == git2::ErrorCode::NotFound =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Full ref name HEAD points to (e.g. refs/heads/main), even if unborn.
    pub fn head_ref_name(&self) -> Result<String> {
        let r = self.git.find_reference("HEAD")?;
        match r.symbolic_target() {
            Some(t) => Ok(t.to_string()),
            None => bail!("HEAD is detached; check out a branch first"),
        }
    }

    pub fn resolve(&self, rev: &str) -> Result<Oid> {
        let obj = self
            .git
            .revparse_single(rev)
            .with_context(|| format!("unknown revision '{rev}'"))?;
        Ok(obj
            .peel_to_commit()
            .with_context(|| format!("'{rev}' is not a commit"))?
            .id())
    }

    /// Tracked files modified (staged or not). Untracked files do not count;
    /// safe checkout never overwrites them.
    pub fn dirty_paths(&self) -> Result<Vec<String>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(false).include_ignored(false);
        let st = self.git.statuses(Some(&mut opts))?;
        Ok(st
            .iter()
            .filter(|e| !e.status().is_empty())
            .filter_map(|e| e.path().map(String::from))
            .collect())
    }

    pub fn diff_trees(&self, old: Option<&Tree>, new: &Tree) -> Result<DiffInfo> {
        let mut o = DiffOptions::new();
        o.context_lines(3);
        let mut diff = self.git.diff_tree_to_tree(old, Some(new), Some(&mut o))?;
        render(&mut diff)
    }

    /// Diff of a commit against its first parent.
    pub fn diff_commit(&self, oid: Oid) -> Result<DiffInfo> {
        let c = self.git.find_commit(oid)?;
        let tree = c.tree()?;
        let parent_tree = if c.parent_count() > 0 {
            Some(c.parent(0)?.tree()?)
        } else {
            None
        };
        self.diff_trees(parent_tree.as_ref(), &tree)
    }

    /// Working tree (including untracked files) vs HEAD.
    pub fn diff_workdir(&self) -> Result<DiffInfo> {
        let head_tree = match self.head_commit()? {
            Some(c) => Some(c.tree()?),
            None => None,
        };
        let mut o = DiffOptions::new();
        o.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let mut diff = self
            .git
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut o))?;
        render(&mut diff)
    }

    /// Stage everything in this repository's working directory and write a tree.
    pub fn stage_all(&self) -> Result<Oid> {
        let mut index = self.git.index()?;
        // Never snapshot build/test byproducts that a test run or agent left behind.
        let mut skip_junk = |path: &Path, _spec: &[u8]| -> i32 {
            let p = path.to_string_lossy();
            let junk = p.contains("__pycache__/")
                || p.ends_with(".pyc")
                || p.starts_with(".pytest_cache/")
                || p.starts_with(".mypy_cache/")
                || p.ends_with(".DS_Store");
            if junk {
                1
            } else {
                0
            }
        };
        index.add_all(["*"].iter(), IndexAddOption::DEFAULT, Some(&mut skip_junk))?;
        index.update_all(["*"].iter(), None)?;
        index.write()?;
        Ok(index.write_tree()?)
    }

    pub fn commit(
        &self,
        update_ref: Option<&str>,
        message: &str,
        tree: Oid,
        parents: &[Oid],
    ) -> Result<Oid> {
        let sig = self.signature();
        let tree = self.git.find_tree(tree)?;
        let parents: Vec<Commit> = parents
            .iter()
            .map(|p| self.git.find_commit(*p))
            .collect::<Result<_, _>>()?;
        let refs: Vec<&Commit> = parents.iter().collect();
        Ok(self
            .git
            .commit(update_ref, &sig, &sig, message, &tree, &refs)?)
    }

    pub fn cherry_pick_in_memory(&self, pick: Oid, onto: Oid) -> Result<MergeOutcome> {
        let pick = self.git.find_commit(pick)?;
        let onto = self.git.find_commit(onto)?;
        let mainline = if pick.parent_count() > 1 { 1 } else { 0 };
        let idx = self.git.cherrypick_commit(&pick, &onto, mainline, None)?;
        self.outcome(idx)
    }

    pub fn revert_in_memory(&self, rev: Oid, onto: Oid) -> Result<MergeOutcome> {
        let rev = self.git.find_commit(rev)?;
        let onto = self.git.find_commit(onto)?;
        let mainline = if rev.parent_count() > 1 { 1 } else { 0 };
        let idx = self.git.revert_commit(&rev, &onto, mainline, None)?;
        self.outcome(idx)
    }

    pub fn merge_in_memory(&self, ours: Oid, theirs: Oid) -> Result<MergeOutcome> {
        let o = self.git.find_commit(ours)?;
        let t = self.git.find_commit(theirs)?;
        let idx = self.git.merge_commits(&o, &t, None)?;
        self.outcome(idx)
    }

    fn outcome(&self, mut idx: git2::Index) -> Result<MergeOutcome> {
        if idx.has_conflicts() {
            let mut paths = vec![];
            let mut ours = vec![];
            for c in idx.conflicts()? {
                let c = c?;
                for e in [&c.our, &c.their, &c.ancestor].into_iter().flatten() {
                    paths.push(String::from_utf8_lossy(&e.path).to_string());
                }
                if let Some(o) = c.our {
                    ours.push(o);
                }
            }
            paths.sort();
            paths.dedup();
            // Resolve every conflict to our side to get a useful starting point.
            for p in &paths {
                let _ = idx.conflict_remove(Path::new(p));
            }
            for mut o in ours {
                o.flags &= !0x3000; // clear the stage bits
                let _ = idx.add(&o);
            }
            let partial = if idx.has_conflicts() {
                None
            } else {
                idx.write_tree_to(&self.git).ok()
            };
            return Ok(MergeOutcome::Conflicts(paths, partial));
        }
        Ok(MergeOutcome::Clean(idx.write_tree_to(&self.git)?))
    }

    pub fn merge_base(&self, a: Oid, b: Oid) -> Result<Oid> {
        Ok(self.git.merge_base(a, b)?)
    }

    /// Non-merge commits reachable from `tip` but not from `base`, oldest first.
    pub fn commits_between(&self, base: Oid, tip: Oid) -> Result<Vec<Oid>> {
        let mut walk = self.git.revwalk()?;
        walk.push(tip)?;
        walk.hide(base)?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)?;
        let mut out = vec![];
        for oid in walk {
            let oid = oid?;
            if self.git.find_commit(oid)?.parent_count() <= 1 {
                out.push(oid);
            }
        }
        Ok(out)
    }

    // ---- notes ----

    pub fn read_note(&self, oid: Oid) -> Option<String> {
        self.git
            .find_note(Some(NOTES_REF), oid)
            .ok()
            .and_then(|n| n.message().map(String::from))
    }

    pub fn write_note(&self, oid: Oid, body: &str) -> Result<()> {
        let sig = self.signature();
        self.git
            .note(&sig, &sig, Some(NOTES_REF), oid, body, true)?;
        Ok(())
    }

    pub fn all_noted_commits(&self) -> Result<Vec<Oid>> {
        let mut out = vec![];
        if let Ok(notes) = self.git.notes(Some(NOTES_REF)) {
            for n in notes {
                let (_note_id, annotated) = n?;
                out.push(annotated);
            }
        }
        Ok(out)
    }

    // ---- worktrees (git CLI) ----

    pub fn add_worktree(&self, path: &Path, commit: Oid) -> Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        run_git(
            &self.workdir,
            &[
                "worktree",
                "add",
                "--detach",
                "--force",
                &path.to_string_lossy(),
                &commit.to_string(),
            ],
        )
    }

    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        if path.exists() {
            let _ = run_git(
                &self.workdir,
                &["worktree", "remove", "--force", &path.to_string_lossy()],
            );
            if path.exists() {
                std::fs::remove_dir_all(path).ok();
            }
        }
        let _ = run_git(&self.workdir, &["worktree", "prune"]);
        Ok(())
    }

    /// Point the worktree at `commit` (detached) and make its files exactly `tree`
    /// (or the commit's tree), removing untracked files.
    pub fn reset_worktree(wt: &Repo, commit: Oid, tree: Option<Oid>) -> Result<()> {
        let c = wt.git.find_commit(commit)?;
        wt.git.reset(c.as_object(), git2::ResetType::Hard, None)?;
        let mut co = CheckoutBuilder::new();
        co.force().remove_untracked(true);
        match tree {
            Some(t) => {
                let tree = wt.git.find_tree(t)?;
                wt.git.checkout_tree(tree.as_object(), Some(&mut co))?;
            }
            None => wt.git.checkout_head(Some(&mut co))?,
        }
        Ok(())
    }

    /// Move `refname` from `expected` to `new` (compare-and-swap) and update the
    /// main working tree safely. Fails without changes if the branch moved or
    /// the checkout would overwrite local modifications.
    pub fn finalize_branch(&self, refname: &str, expected: Option<Oid>, new: Oid) -> Result<()> {
        let current = self.git.refname_to_id(refname).ok();
        if current != expected {
            bail!(
                "branch {refname} moved during the operation (expected {}, found {}); nothing was changed",
                fmt_oid(expected),
                fmt_oid(current)
            );
        }
        let dirty = self.dirty_paths()?;
        if !dirty.is_empty() {
            bail!(
                "working tree has uncommitted changes ({}); refusing to update",
                dirty.join(", ")
            );
        }
        let new_commit = self.git.find_commit(new)?;
        let mut co = CheckoutBuilder::new();
        co.safe();
        self.git
            .checkout_tree(new_commit.as_object(), Some(&mut co))
            .context("safe checkout failed; your working tree was not modified")?;
        match expected {
            Some(exp) => {
                self.git.reference_matching(
                    refname,
                    new,
                    true,
                    exp,
                    "gitent: finalize operation",
                )?;
            }
            None => {
                self.git
                    .reference(refname, new, false, "gitent: finalize operation")?;
            }
        }
        Ok(())
    }
}

fn fmt_oid(o: Option<Oid>) -> String {
    o.map(|o| short(&o.to_string()))
        .unwrap_or_else(|| "nothing".into())
}

pub fn short(s: &str) -> String {
    s.chars().take(10).collect()
}

fn render(diff: &mut git2::Diff) -> Result<DiffInfo> {
    let mut fo = DiffFindOptions::new();
    fo.renames(true);
    diff.find_similar(Some(&mut fo))?;
    let mut files = vec![];
    for d in diff.deltas() {
        let new = d.new_file().path().map(|p| p.to_string_lossy().to_string());
        let old = d.old_file().path().map(|p| p.to_string_lossy().to_string());
        let status = format!("{:?}", d.status()).to_lowercase();
        let path = new.clone().or(old.clone()).unwrap_or_default();
        let old_path = if d.status() == git2::Delta::Renamed {
            old
        } else {
            None
        };
        files.push(FileChange {
            path,
            old_path,
            status,
        });
    }
    let mut patch = String::new();
    diff.print(DiffFormat::Patch, |_d, _h, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            patch.push(line.origin());
        }
        patch.push_str(&String::from_utf8_lossy(line.content()));
        true
    })?;
    Ok(DiffInfo { patch, files })
}

pub fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .context("failed to run `git`")?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
