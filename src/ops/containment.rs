//! Agent containment (ADR 0014 §8): agents edit a linked worktree, but that
//! worktree shares the repository's common git dir. An agent that plants a hook
//! or rewrites `.git/config` (e.g. `core.hooksPath`, `core.fsmonitor`) would get
//! code executed on the user's next git command. Gitent snapshots both before
//! each agent attempt and restores them afterwards if they changed.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    data: Vec<u8>,
    mode: u32,
}

/// The common git dir's `hooks/` (recursively) and `config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirSnapshot {
    git_dir: PathBuf,
    /// Relative paths under `hooks/` → content.
    hooks: BTreeMap<PathBuf, Entry>,
    hooks_existed: bool,
    /// Set when `hooks` itself is a symlink (or a file): its target.
    hooks_link: Option<Entry>,
    config: Option<Entry>,
}

fn mode_of(m: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        let _ = m;
        0o644
    }
}

fn read_entry(p: &Path) -> Option<Entry> {
    let m = std::fs::symlink_metadata(p).ok()?;
    if m.file_type().is_symlink() {
        // Record the link target as content, so retargeting is noticed.
        let t = std::fs::read_link(p).ok()?;
        return Some(Entry {
            data: format!("symlink:{}", t.display()).into_bytes(),
            mode: u32::MAX,
        });
    }
    Some(Entry {
        data: std::fs::read(p).ok()?,
        mode: mode_of(&m),
    })
}

fn walk(root: &Path, rel: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    let Ok(rd) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    for e in rd.flatten() {
        let r = rel.join(e.file_name());
        let p = root.join(&r);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.is_dir() => walk(root, &r, out),
            Ok(_) => {
                if let Some(en) = read_entry(&p) {
                    out.insert(r, en);
                }
            }
            Err(_) => {}
        }
    }
}

impl GitDirSnapshot {
    pub fn take(git_dir: &Path) -> GitDirSnapshot {
        let hooks_dir = git_dir.join("hooks");
        let mut hooks = BTreeMap::new();
        let hooks_existed = hooks_dir.is_dir();
        if hooks_existed {
            walk(&hooks_dir, Path::new(""), &mut hooks);
        }
        let hooks_link = read_link_entry(&hooks_dir);
        GitDirSnapshot {
            git_dir: git_dir.to_path_buf(),
            hooks,
            hooks_existed,
            hooks_link,
            config: read_entry(&git_dir.join("config")),
        }
    }

    /// Human-readable list of what differs from `self` now (empty = unchanged).
    pub fn changes(&self) -> Vec<String> {
        let now = GitDirSnapshot::take(&self.git_dir);
        let mut out = vec![];
        if now.config != self.config {
            out.push("config".to_string());
        }
        if now.hooks_link != self.hooks_link {
            out.push("hooks (replaced by or with a symlink/file)".to_string());
        }
        for (k, v) in &now.hooks {
            if self.hooks.get(k) != Some(v) {
                out.push(format!("hooks/{}", k.display()));
            }
        }
        for k in self.hooks.keys() {
            if !now.hooks.contains_key(k) {
                out.push(format!("hooks/{} (deleted)", k.display()));
            }
        }
        out
    }

    /// Put `hooks/` and `config` back exactly as snapshotted.
    pub fn restore(&self) -> Result<()> {
        let hooks_dir = self.git_dir.join("hooks");
        // `hooks` itself swapped for (or from) a symlink/file: put it back first.
        if read_link_entry(&hooks_dir) != self.hooks_link {
            if let Ok(m) = std::fs::symlink_metadata(&hooks_dir) {
                if m.is_dir() {
                    std::fs::remove_dir_all(&hooks_dir)?;
                } else {
                    std::fs::remove_file(&hooks_dir)?;
                }
            }
            if let Some(e) = &self.hooks_link {
                write_entry(&hooks_dir, e)?;
            }
        }
        let now = GitDirSnapshot::take(&self.git_dir);
        for k in now.hooks.keys() {
            if !self.hooks.contains_key(k) {
                std::fs::remove_file(hooks_dir.join(k)).ok();
            }
        }
        for (k, v) in &self.hooks {
            if now.hooks.get(k) != Some(v) {
                write_entry(&hooks_dir.join(k), v)?;
            }
        }
        if !self.hooks_existed && self.hooks_link.is_none() && hooks_dir.is_dir() {
            std::fs::remove_dir_all(&hooks_dir).ok();
        }
        let cfg = self.git_dir.join("config");
        match &self.config {
            Some(e) if now.config.as_ref() != Some(e) => write_entry(&cfg, e)?,
            None if cfg.exists() => std::fs::remove_file(&cfg)?,
            _ => {}
        }
        Ok(())
    }
}

fn read_link_entry(p: &Path) -> Option<Entry> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if !m.is_dir() => read_entry(p),
        _ => None,
    }
}

fn write_entry(p: &Path, e: &Entry) -> Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(m) = std::fs::symlink_metadata(p) {
        if m.file_type().is_symlink() || m.is_dir() {
            if m.is_dir() {
                std::fs::remove_dir_all(p)?;
            } else {
                std::fs::remove_file(p)?;
            }
        }
    }
    if e.mode == u32::MAX {
        #[cfg(unix)]
        {
            let target = String::from_utf8_lossy(&e.data);
            let target = target.trim_start_matches("symlink:");
            std::os::unix::fs::symlink(target, p)?;
        }
        return Ok(());
    }
    std::fs::write(p, &e.data).with_context(|| format!("restoring {}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(e.mode))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_restores_hook_and_config_changes() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path();
        std::fs::create_dir_all(g.join("hooks")).unwrap();
        std::fs::write(g.join("hooks/pre-commit.sample"), "sample").unwrap();
        std::fs::write(g.join("config"), "[core]\n").unwrap();
        let snap = GitDirSnapshot::take(g);
        assert!(snap.changes().is_empty());

        std::fs::write(g.join("hooks/post-checkout"), "#!/bin/sh\nevil\n").unwrap();
        std::fs::write(g.join("hooks/pre-commit.sample"), "changed").unwrap();
        std::fs::write(g.join("config"), "[core]\nhooksPath = /tmp/x\n").unwrap();
        let ch = snap.changes();
        assert_eq!(ch.len(), 3, "{ch:?}");

        snap.restore().unwrap();
        assert!(snap.changes().is_empty());
        assert!(!g.join("hooks/post-checkout").exists());
        assert_eq!(
            std::fs::read_to_string(g.join("hooks/pre-commit.sample")).unwrap(),
            "sample"
        );
        assert_eq!(
            std::fs::read_to_string(g.join("config")).unwrap(),
            "[core]\n"
        );
    }
}
