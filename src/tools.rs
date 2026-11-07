//! Bounded, path-confined workspace tools for the builtin agent (ADR 0005).

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

pub struct Workspace {
    pub root: PathBuf,
    pub command_timeout: Duration,
}

pub const MAX_OUTPUT: usize = 12_000;

pub const TOOL_DOCS: &str = r#"Tools (reply with exactly one JSON object per turn: {"tool": "<name>", "args": {...}}):
- list_files {"path": "dir (optional)"}                    list files recursively
- read_file  {"path": "file", "start": 1, "end": 200}      read lines (1-based, inclusive; optional range)
- search     {"pattern": "regex", "path": "dir (optional)"} grep the workspace
- write_file {"path": "file", "content": "full text"}      create/overwrite a file
- replace    {"path": "file", "old": "exact text", "new": "replacement"}  replace one exact, unique occurrence
- run        {"command": "shell command"}                  run a command in the workspace (bounded time)
- diff       {}                                            show your changes so far (git diff)
- finish     {"summary": "what you did and why"}           end when done"#;

impl Workspace {
    /// Resolve a relative path inside the workspace; reject escapes and `.git`.
    pub fn resolve(&self, rel: &str) -> Result<PathBuf> {
        let rel = rel.trim();
        let p = Path::new(if rel.is_empty() { "." } else { rel });
        if p.is_absolute() {
            bail!("absolute paths are not allowed: {rel}");
        }
        let mut out = self.root.clone();
        for c in p.components() {
            match c {
                Component::Normal(s) => {
                    if s == ".git" {
                        bail!("access to .git is not allowed");
                    }
                    out.push(s)
                }
                Component::CurDir => {}
                _ => bail!("path escapes the workspace: {rel}"),
            }
        }
        Ok(out)
    }

    pub fn call(&self, tool: &str, args: &Value) -> Result<String> {
        let s = |k: &str| {
            args.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        match tool {
            "list_files" => {
                let base = self.resolve(&s("path"))?;
                let files = crate::diff::source_files(&base);
                let rel_base = base
                    .strip_prefix(&self.root)
                    .unwrap_or(Path::new(""))
                    .to_path_buf();
                Ok(files
                    .iter()
                    .map(|f| rel_base.join(f).to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "read_file" => {
                let p = self.resolve(&s("path"))?;
                let text = std::fs::read_to_string(&p)
                    .map_err(|e| anyhow!("cannot read {}: {e}", s("path")))?;
                let lines: Vec<&str> = text.lines().collect();
                let start = args
                    .get("start")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1)
                    .max(1) as usize;
                let end = args
                    .get("end")
                    .and_then(|v| v.as_u64())
                    .map(|e| e as usize)
                    .unwrap_or(lines.len())
                    .min(lines.len());
                let mut out = String::new();
                for (i, l) in lines.iter().enumerate().take(end).skip(start - 1) {
                    out.push_str(&format!("{:>5} {}\n", i + 1, l));
                }
                Ok(crate::proc::tail(&out, MAX_OUTPUT))
            }
            "search" => {
                let re = regex::Regex::new(&s("pattern"))?;
                let base = self.resolve(&s("path"))?;
                let mut out = String::new();
                for f in crate::diff::source_files(&base) {
                    if let Ok(text) = std::fs::read_to_string(base.join(&f)) {
                        for (i, l) in text.lines().enumerate() {
                            if re.is_match(l) {
                                let rel = base.join(&f);
                                let rel = rel.strip_prefix(&self.root).unwrap_or(&rel);
                                out.push_str(&format!(
                                    "{}:{}: {}\n",
                                    rel.display(),
                                    i + 1,
                                    l.trim()
                                ));
                            }
                        }
                    }
                    if out.len() > MAX_OUTPUT {
                        break;
                    }
                }
                Ok(if out.is_empty() {
                    "(no matches)".into()
                } else {
                    crate::proc::tail(&out, MAX_OUTPUT)
                })
            }
            "write_file" => {
                let p = self.resolve(&s("path"))?;
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let content = s("content");
                std::fs::write(&p, &content)?;
                Ok(format!("wrote {} bytes to {}", content.len(), s("path")))
            }
            "replace" => {
                let p = self.resolve(&s("path"))?;
                let text = std::fs::read_to_string(&p)?;
                let old = s("old");
                if old.is_empty() {
                    bail!("`old` must not be empty");
                }
                let n = text.matches(&old).count();
                if n != 1 {
                    bail!("`old` must occur exactly once in {} (found {n})", s("path"));
                }
                std::fs::write(&p, text.replacen(&old, &s("new"), 1))?;
                Ok(format!("replaced 1 occurrence in {}", s("path")))
            }
            "run" => {
                // A command the LLM chose, run on repository code: no keys, no tokens.
                let o = crate::proc::run_shell_with(
                    crate::proc::EnvPolicy::RepoCode,
                    &s("command"),
                    &self.root,
                    None,
                    &[],
                    self.command_timeout,
                )?;
                Ok(format!(
                    "exit={:?} timed_out={}\n{}",
                    o.code,
                    o.timed_out,
                    o.combined_tail(MAX_OUTPUT)
                ))
            }
            "diff" => {
                let o = crate::proc::run_shell(
                    "git add -A -N . >/dev/null 2>&1; git --no-pager diff",
                    &self.root,
                    None,
                    &[],
                    Duration::from_secs(30),
                )?;
                Ok(if o.stdout.trim().is_empty() {
                    "(no changes)".into()
                } else {
                    crate::proc::tail(&o.stdout, MAX_OUTPUT)
                })
            }
            other => bail!("unknown tool `{other}`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_confinement() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace {
            root: d.path().to_path_buf(),
            command_timeout: Duration::from_secs(5),
        };
        assert!(ws.resolve("../x").is_err());
        assert!(ws.resolve("/etc/passwd").is_err());
        assert!(ws.resolve(".git/config").is_err());
        assert!(ws.resolve("a/../../b").is_err());
        assert_eq!(ws.resolve("a/./b").unwrap(), d.path().join("a/b"));
    }

    #[test]
    fn edit_tools() {
        let d = tempfile::tempdir().unwrap();
        let ws = Workspace {
            root: d.path().to_path_buf(),
            command_timeout: Duration::from_secs(5),
        };
        ws.call(
            "write_file",
            &serde_json::json!({"path":"src/a.py","content":"x = 1\ny = 2\n"}),
        )
        .unwrap();
        assert!(ws
            .call(
                "replace",
                &serde_json::json!({"path":"src/a.py","old":"y = 2","new":"y = 3"})
            )
            .is_ok());
        assert!(ws
            .call(
                "replace",
                &serde_json::json!({"path":"src/a.py","old":"nope","new":""})
            )
            .is_err());
        let r = ws
            .call(
                "read_file",
                &serde_json::json!({"path":"src/a.py","start":2}),
            )
            .unwrap();
        assert!(r.contains("y = 3") && !r.contains("x = 1"));
        assert!(ws
            .call("search", &serde_json::json!({"pattern":"y ="}))
            .unwrap()
            .contains("src/a.py:2"));
        assert!(ws
            .call("list_files", &serde_json::json!({}))
            .unwrap()
            .contains("src/a.py"));
    }
}
