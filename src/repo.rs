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
    /// Default: `$TMPDIR/gitent-<uid>/<repo-hash>/`, private to the user
    /// (mode 0700, created by [`Repo::add_worktree`]; ADR 0014 §8).
    pub fn worktree_root(&self, configured: &str) -> PathBuf {
        if !configured.trim().is_empty() {
            let p = PathBuf::from(configured);
            return if p.is_absolute() {
                p
            } else {
                self.workdir.join(p)
            };
        }
        let canon = self
            .git_dir
            .canonicalize()
            .unwrap_or_else(|_| self.git_dir.clone());
        let id = Oid::hash_object(git2::ObjectType::Blob, canon.to_string_lossy().as_bytes())
            .map(|o| o.to_string()[..16].to_string())
            .unwrap_or_else(|_| "repo".into());
        user_tmp_base().join(id)
    }

    /// Whether `git worktree list` (libgit2's view of it) knows `path` as a
    /// linked worktree of this repository.
    pub fn is_known_worktree(&self, path: &Path) -> bool {
        let want = canonical(path);
        let Ok(names) = self.git.worktrees() else {
            return false;
        };
        names.iter().flatten().any(|n| {
            self.git
                .find_worktree(n)
                .map(|w| canonical(w.path()) == want)
                .unwrap_or(false)
        })
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
            let base = user_tmp_base();
            if let Ok(rest) = p.strip_prefix(&base) {
                ensure_private_dir(&base)?;
                let mut d = base.clone();
                for c in rest.components() {
                    d.push(c);
                    ensure_private_dir(&d)?;
                }
            } else {
                std::fs::create_dir_all(p)?;
            }
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

    /// Remove a temporary worktree under the default worktree root.
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        self.remove_worktree_in(&self.worktree_root(""), path)
    }

    /// Remove the worktree at `path`, which must lie under `root` and be known
    /// to `git worktree list` (ADR 0014 §3). Anything else is refused, so a
    /// crafted path in the op journal can never delete arbitrary directories.
    /// Empty parent directories below `root` (and a default root itself) are
    /// removed too, so nothing is left behind in the temp directory.
    pub fn remove_worktree_in(&self, root: &Path, path: &Path) -> Result<()> {
        if path.exists() {
            let under_root = is_strictly_under(root, path);
            let known = self.is_known_worktree(path);
            if !under_root || !known || canonical(path) == canonical(&self.workdir) {
                bail!(
                    "refusing to remove {}: not a gitent worktree of this repository under {}",
                    path.display(),
                    root.display()
                );
            }
            let _ = run_git(
                &self.workdir,
                &["worktree", "remove", "--force", &path.to_string_lossy()],
            );
            if path.exists() {
                std::fs::remove_dir_all(path).ok();
            }
        }
        let _ = run_git(&self.workdir, &["worktree", "prune"]);
        // Tidy empty parents (remove_dir only removes empty directories).
        let remove_root = is_strictly_under(&user_tmp_base(), root);
        let mut d = path.parent().map(Path::to_path_buf);
        while let Some(p) = d {
            let inside =
                is_strictly_under(root, &p) || (remove_root && canonical(&p) == canonical(root));
            if !inside || std::fs::remove_dir(&p).is_err() {
                break;
            }
            d = p.parent().map(Path::to_path_buf);
        }
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

/// `$TMPDIR/gitent-<uid>`: the per-user parent of default worktree roots.
pub fn user_tmp_base() -> PathBuf {
    std::env::temp_dir().join(format!("gitent-{}", user_id()))
}

#[cfg(unix)]
fn user_id() -> String {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }.to_string()
}

#[cfg(not(unix))]
fn user_id() -> String {
    std::env::var("USERNAME")
        .unwrap_or_else(|_| "user".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

/// Create `dir` (one level) as a private directory, or check that an existing
/// one is a real directory owned by us; tighten its mode to 0700.
fn ensure_private_dir(dir: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut b = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            match b.create(dir) {
                Ok(()) => return Ok(()),
                // Lost a race: fall through to the ownership check.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
            }
        }
        Err(e) => return Err(e).with_context(|| format!("inspecting {}", dir.display())),
        Ok(_) => {}
    }
    let m = std::fs::symlink_metadata(dir)?;
    if !m.file_type().is_dir() {
        bail!(
            "refusing to use {}: not a directory (or a symlink)",
            dir.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // SAFETY: see user_id.
        let me = unsafe { libc::getuid() };
        if m.uid() != me {
            bail!(
                "refusing to use {}: owned by uid {}, not {me}",
                dir.display(),
                m.uid()
            );
        }
        if m.mode() & 0o077 != 0 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// `path` is inside `root` (and not `root` itself), after resolving symlinks.
fn is_strictly_under(root: &Path, path: &Path) -> bool {
    let (r, p) = (canonical(root), canonical(path));
    p != r && p.starts_with(&r)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn repo_with_commit(dir: &Path) -> (Repo, Oid) {
        let r = Repo::init(&dir.join("repo")).unwrap();
        std::fs::write(r.workdir.join("a.txt"), "a\n").unwrap();
        let tree = r.stage_all().unwrap();
        let c = r.commit(Some("HEAD"), "init", tree, &[]).unwrap();
        (r, c)
    }

    #[test]
    fn cleanup_only_removes_known_worktrees_under_the_root() {
        let d = tempfile::tempdir().unwrap();
        let (r, c) = repo_with_commit(d.path());
        let root = d.path().join("wt");
        let wt = root.join("tmp").join("op_x");
        r.add_worktree(&wt, c).unwrap();
        assert!(r.is_known_worktree(&wt));

        // A directory under the root that git does not know: refused, kept.
        let stray = root.join("op_stray");
        std::fs::create_dir_all(&stray).unwrap();
        assert!(r.remove_worktree_in(&root, &stray).is_err());
        assert!(stray.exists());
        // Outside the root (even the main working tree): refused, kept.
        let outside = d.path().join("precious");
        std::fs::create_dir_all(&outside).unwrap();
        assert!(r.remove_worktree_in(&root, &outside).is_err());
        assert!(outside.exists());
        assert!(r.remove_worktree_in(d.path(), &r.workdir).is_err());
        assert!(r.workdir.join("a.txt").exists());
        // A known worktree outside the given root: refused.
        assert!(r.remove_worktree_in(&outside, &wt).is_err());
        assert!(wt.exists());

        // The real one goes, with its now-empty parent; the root stays.
        r.remove_worktree_in(&root, &wt).unwrap();
        assert!(!wt.exists() && !root.join("tmp").exists());
        assert!(root.exists() && stray.exists());
        assert!(!r.is_known_worktree(&wt));
    }

    #[cfg(unix)]
    #[test]
    fn private_dirs_are_0700_and_must_be_ours() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("gitent-x");
        ensure_private_dir(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        // An existing, too-open directory is tightened.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(&p).unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A symlink planted in its place is refused.
        let link = d.path().join("gitent-link");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());
        assert!(user_tmp_base()
            .to_string_lossy()
            .ends_with(&format!("gitent-{}", user_id())));
    }
}
