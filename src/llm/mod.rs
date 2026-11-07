//! LLM provider abstraction (ADR 0004). Text in, text out.

mod anthropic;
mod command;
mod openai;
mod scripted;

use crate::config::LlmConfig;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn user(s: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: s.into(),
        }
    }
    pub fn assistant(s: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: s.into(),
        }
    }
}

pub trait Provider: Send + Sync {
    /// Human-readable identity, recorded in provenance (e.g. "anthropic:claude-opus-5-5").
    fn name(&self) -> String;
    fn complete(&self, system: &str, messages: &[Message]) -> Result<String>;
    /// Like `complete`, for a provider that can look at files (an agentic CLI): run it in
    /// `dir`, the tree being judged, not the user's checkout. API providers ignore `dir`.
    fn complete_in(&self, system: &str, messages: &[Message], dir: &Path) -> Result<String> {
        let _ = dir;
        self.complete(system, messages)
    }
}

/// Build the configured provider; `Ok(None)` when provider = "none".
pub fn from_config(cfg: &LlmConfig, base_dir: &Path) -> Result<Option<Arc<dyn Provider>>> {
    let p: Arc<dyn Provider> = match cfg.provider.as_str() {
        "" | "none" => return Ok(None),
        "anthropic" => Arc::new(anthropic::Anthropic::new(cfg)?),
        "openai" => Arc::new(openai::OpenAi::new(cfg)?),
        "command" => {
            if cfg.command.trim().is_empty() {
                bail!("llm.provider = \"command\" requires llm.command");
            }
            Arc::new(command::CommandProvider::new(cfg, base_dir))
        }
        "scripted" => {
            let path = base_dir.join(&cfg.script);
            Arc::new(scripted::Scripted::new(path))
        }
        other => bail!(
            "unknown llm provider '{other}' (expected anthropic|openai|command|scripted|none)"
        ),
    };
    Ok(Some(p))
}

/// Extract the first JSON object from model output (tolerates prose and code fences).
pub fn extract_json(text: &str) -> Result<serde_json::Value> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if v.is_object() {
            return Ok(v);
        }
    }
    // Scan for balanced {...} candidates, honouring strings.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = balanced_end(text, i) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[i..=end]) {
                    if v.is_object() {
                        return Ok(v);
                    }
                }
            }
        }
        i += 1;
    }
    Err(anyhow!(
        "no JSON object found in model output: {}",
        crate::proc::tail(text, 400)
    ))
}

fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (off, ch) in text[start..].char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
            continue;
        }
        match ch {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + off);
                }
            }
            _ => {}
        }
    }
    None
}

/// Flatten a conversation into one prompt (for providers without chat structure).
pub fn flatten(system: &str, messages: &[Message]) -> String {
    if messages.len() == 1 {
        return format!("{system}\n\n{}", messages[0].content);
    }
    let mut s = format!("# SYSTEM\n{system}\n\n# CONVERSATION\n");
    for m in messages {
        let who = match m.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
        };
        s.push_str(&format!("\n## {who}\n{}\n", m.content));
    }
    s.push_str("\n# Reply as ASSISTANT to the last USER message. Output only the reply.\n");
    s
}

pub(crate) fn http_agent(timeout_secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_variants() {
        assert_eq!(extract_json(r#"{"a":1}"#).unwrap()["a"], 1);
        let t = "Sure! Here it is:\n```json\n{\"title\": \"x {y}\", \"n\": {\"k\": 2}}\n```\nDone.";
        let v = extract_json(t).unwrap();
        assert_eq!(v["title"], "x {y}");
        assert_eq!(v["n"]["k"], 2);
        assert!(extract_json("no json here").is_err());
        assert_eq!(
            extract_json("bad {oops} then {\"ok\":true}").unwrap()["ok"],
            true
        );
    }

    #[test]
    fn flatten_single_and_multi() {
        assert_eq!(flatten("S", &[Message::user("U")]), "S\n\nU");
        let f = flatten(
            "S",
            &[
                Message::user("a"),
                Message::assistant("b"),
                Message::user("c"),
            ],
        );
        assert!(f.contains("## ASSISTANT\nb") && f.ends_with("Output only the reply.\n"));
    }
}
