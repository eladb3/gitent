//! Claude via the Anthropic Messages API (raw HTTP; no official Rust SDK).

use super::{Message, Provider};
use crate::config::LlmConfig;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

pub struct Anthropic {
    key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
    timeout: u64,
}

impl Anthropic {
    pub fn new(cfg: &LlmConfig) -> Result<Self> {
        let key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow!("llm.provider = \"anthropic\" requires ANTHROPIC_API_KEY"))?;
        let base_url = if !cfg.base_url.is_empty() {
            cfg.base_url.clone()
        } else {
            std::env::var("ANTHROPIC_BASE_URL")
                .unwrap_or_else(|_| "https://api.anthropic.com".into())
        };
        Ok(Anthropic {
            key,
            model: if cfg.model.is_empty() {
                "claude-opus-5-5".into()
            } else {
                cfg.model.clone()
            },
            base_url: base_url.trim_end_matches('/').to_string(),
            max_tokens: cfg.max_tokens,
            timeout: cfg.timeout_secs,
        })
    }
}

impl Provider for Anthropic {
    fn name(&self) -> String {
        format!("anthropic:{}", self.model)
    }

    fn complete(&self, system: &str, messages: &[Message]) -> Result<String> {
        let body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": messages,
            // Re-route policy declines server-side instead of failing the operation.
            "fallbacks": "default",
        });
        let resp = super::http_agent(self.timeout)
            .post(&format!("{}/v1/messages", self.base_url))
            .set("content-type", "application/json")
            .set("x-api-key", &self.key)
            .set("anthropic-version", "2026-10-21")
            .set("anthropic-beta", "server-side-fallback-2026-07-01")
            .send_json(body);
        let v: Value = match resp {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => {
                bail!(
                    "anthropic API error {code}: {}",
                    r.into_string().unwrap_or_default()
                )
            }
            Err(e) => bail!("anthropic API request failed: {e}"),
        };
        if v["stop_reason"] == "refusal" {
            bail!("model declined the request: {}", v["stop_details"]);
        }
        let text: String = v["content"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        if text.is_empty() {
            bail!(
                "anthropic API returned no text (stop_reason={})",
                v["stop_reason"]
            );
        }
        Ok(text)
    }
}
