//! Bounded subprocess execution.

use anyhow::{Context, Result};
use std::cell::Cell;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
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

/// How long to keep reading output after the main process ended (or was killed)
/// before giving up on descendants that still hold the pipes.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Run `sh -c <cmd>` in `cwd`, feeding `stdin`, with a timeout.
///
/// The command runs in its own process group (ADR 0014 §7). On timeout the
/// whole group is killed, so test runners' worker processes cannot outlive it.
/// Output is collected by reader threads; if descendants keep the pipes open
/// after the main process ended, the group is killed and whatever was read so
/// far is returned.
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
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    let mut child = c
        .spawn()
        .with_context(|| format!("failed to spawn `{cmd}`"))?;
    let pgid = child.id();
    if let Some(input) = stdin {
        let mut si = child.stdin.take().unwrap();
        let input = input.to_string();
        std::thread::spawn(move || {
            let _ = si.write_all(input.as_bytes());
        });
    }
    let out = Collector::spawn(child.stdout.take().unwrap());
    let err = Collector::spawn(child.stderr.take().unwrap());
    let (code, timed_out) = match child.wait_timeout(timeout)? {
        Some(st) => (st.code(), false),
        None => {
            kill_group(pgid);
            let _ = child.kill();
            let _ = child.wait();
            (None, true)
        }
    };
    // Descendants may still hold the pipes: give them a moment, then kill the group.
    let deadline = Instant::now() + DRAIN_GRACE;
    if !(out.wait_until(deadline) && err.wait_until(deadline)) {
        kill_group(pgid);
        let d2 = Instant::now() + DRAIN_GRACE;
        out.wait_until(d2);
        err.wait_until(d2);
    }
    Ok(ProcOutput {
        code,
        timed_out,
        stdout: out.take(),
        stderr: err.take(),
        duration_ms: start.elapsed().as_millis(),
    })
}

/// Kill every process in the group led by `pgid`.
fn kill_group(pgid: u32) {
    #[cfg(unix)]
    {
        if let Ok(p) = i32::try_from(pgid) {
            if p > 0 {
                // SAFETY: plain syscall; a negative pid addresses the process group.
                unsafe {
                    libc::kill(-p, libc::SIGKILL);
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = pgid;
}

/// Reads a pipe to the end on a thread, keeping what was read available even
/// if the thread never finishes.
struct Collector {
    buf: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
    finished: Cell<bool>,
}

impl Collector {
    fn spawn<R: Read + Send + 'static>(mut r: R) -> Collector {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let b2 = buf.clone();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                match r.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => b2.lock().unwrap().extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = tx.send(());
        });
        Collector {
            buf,
            done: rx,
            finished: Cell::new(false),
        }
    }

    /// True once the pipe reached EOF (waits at most until `deadline`).
    fn wait_until(&self, deadline: Instant) -> bool {
        if self.finished.get() {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let ok = !matches!(
            self.done.recv_timeout(left),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        self.finished.set(ok);
        ok
    }

    fn take(&self) -> String {
        let b = std::mem::take(&mut *self.buf.lock().unwrap());
        String::from_utf8_lossy(&b).to_string()
    }
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
    fn timeout_kills_the_whole_process_group() {
        let d = std::env::temp_dir();
        let t = Instant::now();
        let o = run_shell("sleep 30 & sleep 30", &d, None, &[], Duration::from_secs(1)).unwrap();
        assert!(o.timed_out && !o.success());
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "took {:?}: a grandchild kept the pipes open",
            t.elapsed()
        );
    }

    #[test]
    fn background_grandchild_does_not_block_after_exit() {
        let d = std::env::temp_dir();
        let t = Instant::now();
        let o = run_shell(
            "sleep 30 & echo done",
            &d,
            None,
            &[],
            Duration::from_secs(60),
        )
        .unwrap();
        assert!(o.success(), "{o:?}");
        assert_eq!(o.stdout.trim(), "done");
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "took {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn tail_keeps_end() {
        assert_eq!(tail("abcdef", 3), "…(truncated)…\ndef");
    }
}
