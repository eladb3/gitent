//! Any OpenAI-compatible chat completions endpoint (OpenAI, Ollama, vLLM, LM Studio, …).

use super::{Message, Provider, Role};
use crate::config::LlmConfig;
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub struct OpenAi {
    key: Option<String>,
    model: String,
    base_url: String,
    timeout: u64,
}

impl OpenAi {
    pub fn new(cfg: &LlmConfig) -> Result<Self> {
        if cfg.model.is_empty() {
            bail!("llm.provider = \"openai\" requires llm.model");
        }
        let base = super::resolve_base_url(
            super::env_base_url("OPENAI_BASE_URL"),
            &cfg.base_url,
            "https://api.openai.com/v1",
        );
        let key = std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.is_empty());
        if key.is_some() {
            super::check_key_destination(&base)?;
        }
        Ok(OpenAi {
            key,
            model: cfg.model.clone(),
            base_url: base,
            timeout: cfg.timeout_secs,
        })
    }
}

impl Provider for OpenAi {
    fn name(&self) -> String {
        format!("openai:{}", self.model)
    }

    fn complete(&self, system: &str, messages: &[Message]) -> Result<String> {
        let mut msgs = vec![json!({"role": "system", "content": system})];
        for m in messages {
            let role = if m.role == Role::User {
                "user"
            } else {
                "assistant"
            };
            msgs.push(json!({"role": role, "content": m.content}));
        }
        let mut req = super::http_agent(self.timeout)
            .post(&format!("{}/chat/completions", self.base_url))
            .set("content-type", "application/json");
        if let Some(k) = &self.key {
            req = req.set("authorization", &format!("Bearer {k}"));
        }
        let v: Value = match req.send_json(json!({"model": self.model, "messages": msgs})) {
            Ok(r) => r.into_json()?,
            Err(ureq::Error::Status(code, r)) => bail!(
                "openai API error {code}: {}",
                r.into_string().unwrap_or_default()
            ),
            Err(e) => bail!("openai API request failed: {e}"),
        };
        match v["choices"][0]["message"]["content"].as_str() {
            Some(s) if !s.is_empty() => Ok(s.to_string()),
            _ => bail!("openai API returned no content: {v}"),
        }
    }
}
