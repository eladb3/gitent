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

/// Resolve the API base URL: the environment (`GITENT_LLM_BASE_URL` via
/// `Config::apply_env`, or the vendor variable) beats the config file
/// (ADR 0014 §5), then the vendor default.
pub(crate) fn resolve_base_url(
    vendor_env: Option<String>,
    configured: &str,
    default: &str,
) -> String {
    let u = match vendor_env.filter(|v| !v.trim().is_empty()) {
        Some(v) => v,
        None if !configured.trim().is_empty() => configured.to_string(),
        None => default.to_string(),
    };
    u.trim().trim_end_matches('/').to_string()
}

/// `GITENT_LLM_BASE_URL`, else the vendor variable (e.g. `ANTHROPIC_BASE_URL`).
pub(crate) fn env_base_url(vendor_var: &str) -> Option<String> {
    ["GITENT_LLM_BASE_URL", vendor_var]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
}

/// Refuse to send an API key anywhere but `https://`, or plain `http://` on loopback.
pub(crate) fn check_key_destination(url: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        if crate::http::is_loopback_host(host_only(authority)) {
            return Ok(());
        }
        bail!(
            "refusing to send an API key over plain http to {url}: use https, or a loopback address for a local proxy"
        );
    }
    bail!("unsupported LLM base URL {url:?} (expected https://…)")
}

fn host_only(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    authority.split(':').next().unwrap_or("")
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
    fn env_base_url_beats_config() {
        assert_eq!(
            resolve_base_url(
                Some("https://proxy.corp/".into()),
                "https://evil.example",
                "https://d"
            ),
            "https://proxy.corp"
        );
        assert_eq!(
            resolve_base_url(None, "https://cfg", "https://d"),
            "https://cfg"
        );
        assert_eq!(
            resolve_base_url(Some(" ".into()), "", "https://d"),
            "https://d"
        );
    }

    #[test]
    fn api_keys_only_go_to_https_or_loopback() {
        for ok in [
            "https://api.anthropic.com",
            "HTTPS://x.example/v1",
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:9000/v1",
        ] {
            assert!(check_key_destination(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://evil.example",
            "http://10.0.0.5:8000/v1",
            "http://localhost.evil.example",
            "http://localhost@evil.example/",
            "ftp://x",
            "evil.example",
        ] {
            assert!(check_key_destination(bad).is_err(), "{bad}");
        }
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
