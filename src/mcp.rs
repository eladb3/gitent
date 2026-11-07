//! MCP server over stdio (newline-delimited JSON-RPC 2.0).

use crate::api::{self, Surface};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn handle(repo: &Path, req: &Value) -> Option<Value> {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    // Notifications get no response.
    id.as_ref()?;
    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "gitent", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Gitent is semantic version control on top of git. Use gitent_commit (pass your own `intent`) \
                instead of git commit; use gitent_cherry_pick / gitent_merge / gitent_rebase / gitent_revert to move \
                intentions between branches. When a result has state=awaiting_host, read the brief, edit files ONLY in the \
                given worktree, then call gitent_continue."
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            Ok(json!({"tools": api::tool_defs(), "_meta": {"gitent/schema": api::TOOLS_SCHEMA}}))
        }
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match api::call(repo, name, &args, Surface::Agent) {
                Ok(v) => Ok(json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                    "structuredContent": v,
                    "isError": is_failed(&v),
                })),
                Err(e) => Ok(
                    json!({"content": [{"type": "text", "text": format!("error: {e:#}")}], "isError": true}),
                ),
            }
        }
        "resources/list" => Ok(json!({"resources": []})),
        "prompts/list" => Ok(json!({"prompts": []})),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, msg)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
        }
    })
}

pub fn is_failed(v: &Value) -> bool {
    v.get("state").and_then(|s| s.as_str()) == Some("failed")
}

pub fn serve_stdio(repo: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(repo, &req),
            Err(e) => Some(
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}}),
            ),
        };
        if let Some(r) = resp {
            writeln!(out, "{}", serde_json::to_string(&r)?)?;
            out.flush()?;
        }
    }
    Ok(())
}
