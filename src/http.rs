//! Minimal HTTP API: `GET /v1/tools`, `POST /v1/tools/<name>` (JSON body = arguments),
//! `POST /mcp` (one JSON-RPC message). Binds to localhost by default; no auth.

use crate::api::{self, Surface};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;

pub fn serve(repo: &Path, addr: &str) -> Result<()> {
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("cannot bind {addr}: {e}"))?;
    eprintln!(
        "gitent: serving {} on http://{}",
        repo.display(),
        server.server_addr()
    );
    for mut req in server.incoming_requests() {
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let (code, v) = route(repo, req.method(), req.url(), &body);
        let resp =
            tiny_http::Response::from_string(serde_json::to_string_pretty(&v).unwrap_or_default())
                .with_status_code(code)
                .with_header(
                    tiny_http::Header::from_bytes("content-type", "application/json").unwrap(),
                );
        let _ = req.respond(resp);
    }
    Ok(())
}

pub fn route(repo: &Path, method: &tiny_http::Method, url: &str, body: &str) -> (u16, Value) {
    use tiny_http::Method::*;
    let path = url.split('?').next().unwrap_or("");
    match (method, path) {
        (Get, "/v1/tools") => (200, json!({"tools": api::tool_defs()})),
        (Get, "/health") => (200, json!({"ok": true})),
        (Post, "/mcp") => match serde_json::from_str::<Value>(body) {
            Ok(req) => (200, crate::mcp::handle(repo, &req).unwrap_or(json!({}))),
            Err(e) => (400, json!({"error": format!("invalid JSON: {e}")})),
        },
        (Post, p) if p.starts_with("/v1/tools/") => {
            let name = &p["/v1/tools/".len()..];
            let args: Value = if body.trim().is_empty() {
                json!({})
            } else {
                match serde_json::from_str(body) {
                    Ok(v) => v,
                    Err(e) => return (400, json!({"error": format!("invalid JSON: {e}")})),
                }
            };
            match api::call(repo, name, &args, Surface::Agent) {
                Ok(v) => (if crate::mcp::is_failed(&v) { 422 } else { 200 }, v),
                Err(e) => (400, json!({"error": format!("{e:#}")})),
            }
        }
        _ => (404, json!({"error": "not found"})),
    }
}
