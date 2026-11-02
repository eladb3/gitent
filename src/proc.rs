//! Bounded subprocess execution.

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcOutput {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
}

impl ProcOutput {
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == Some(0)
    }

    /// stdout+stderr, truncated from the front to keep the tail (where errors are).
    pub fn combined_tail(&self, max: usize) -> String {
        let s = format!("{}{}", self.stdout, self.stderr);
        tail(&s, max)
    }
}

pub fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…(truncated)…\n{}", &s[start..])
}

/// Run `sh -c <cmd>` in `cwd`, feeding `stdin`, with a timeout.
pub fn run_shell(
    cmd: &str,
    cwd: &Path,
    stdin: Option<&str>,
    env: &[(&str, String)],
    timeout: Duration,
) -> Result<ProcOutput> {
    let start = Instant::now();
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(cwd);
    c.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        c.env(k, v);
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("failed to spawn `{cmd}`"))?;
    if let Some(input) = stdin {
        let mut si = child.stdin.take().unwrap();
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(input.as_bytes());
        });
    }
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t1 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t2 = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let (code, timed_out) = match child.wait_timeout(timeout)? {
        Some(st) => (st.code(), false),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            (None, true)
        }
    };
    let stdout = String::from_utf8_lossy(&t1.join().unwrap_or_default()).to_string();
    let stderr = String::from_utf8_lossy(&t2.join().unwrap_or_default()).to_string();
    Ok(ProcOutput {
        code,
        timed_out,
        stdout,
        stderr,
        duration_ms: start.elapsed().as_millis(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_with_stdin_and_timeout() {
        let d = std::env::temp_dir();
        let o = run_shell(
            "cat; echo err >&2",
            &d,
            Some("hello"),
            &[],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(o.success());
        assert_eq!(o.stdout, "hello");
        assert_eq!(o.stderr.trim(), "err");
        let o = run_shell("sleep 5", &d, None, &[], Duration::from_millis(200)).unwrap();
        assert!(o.timed_out && !o.success());
    }

    #[test]
    fn tail_keeps_end() {
        assert_eq!(tail("abcdef", 3), "…(truncated)…\ndef");
    }
}
