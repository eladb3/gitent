//! Deterministic replay provider for tests and demos.
//!
//! The script is a JSON array. Each entry is either a string (matches any
//! request) or `{"when": "<substring>", "response": "<text>"}` (matches when
//! the substring occurs in the system prompt or the last message). Each entry
//! is used at most once; consumption state persists next to the script
//! (`<script>.state`) so it works across `gitent` invocations.

use super::{Message, Provider};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

pub struct Scripted {
    path: PathBuf,
}

impl Scripted {
    pub fn new(path: PathBuf) -> Self {
        Scripted { path }
    }

    fn state_path(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".state");
        PathBuf::from(p)
    }
}

impl Provider for Scripted {
    fn name(&self) -> String {
        "scripted".into()
    }

    fn complete(&self, system: &str, messages: &[Message]) -> Result<String> {
        let text = std::fs::read_to_string(&self.path)
            .with_context(|| format!("reading script {}", self.path.display()))?;
        let entries: Vec<Value> = serde_json::from_str(&text)?;
        let mut used: Vec<usize> = std::fs::read_to_string(self.state_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let last = messages.last().map(|m| m.content.as_str()).unwrap_or("");
        for (i, e) in entries.iter().enumerate() {
            if used.contains(&i) {
                continue;
            }
            let (when, resp) = match e {
                Value::String(s) => (None, s.clone()),
                Value::Object(o) => (
                    o.get("when").and_then(|w| w.as_str()).map(String::from),
                    match o.get("response") {
                        Some(Value::String(s)) => s.clone(),
                        Some(v) => v.to_string(),
                        None => continue,
                    },
                ),
                _ => continue,
            };
            if let Some(w) = when {
                if !system.contains(&w) && !last.contains(&w) {
                    continue;
                }
            }
            used.push(i);
            std::fs::write(self.state_path(), serde_json::to_string(&used)?)?;
            return Ok(resp);
        }
        Err(anyhow!(
            "scripted provider: no remaining response matches this request"
        ))
    }
}
