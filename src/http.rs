//! Minimal HTTP API: `GET /v1/tools`, `POST /v1/tools/<name>` (JSON body = arguments),
//! `POST /mcp` (one JSON-RPC message). Binds to localhost by default.
//!
//! Every request must carry `Authorization: Bearer <token>` (from `GITENT_TOKEN`,
//! or generated at start-up and printed once on stderr), a loopback `Host`, and
//! no foreign `Origin` — so neither other local users' web pages (CSRF) nor DNS
//! rebinding can drive the tools (ADR 0014 §1).

use crate::api::{self, Surface};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;

/// Largest request body accepted.
const MAX_BODY: u64 = 16 * 1024 * 1024;

/// The token clients must present, and whether it was generated (not from env).
pub fn token_from_env() -> (String, bool) {
    match std::env::var("GITENT_TOKEN") {
        Ok(t) if !t.trim().is_empty() => (t.trim().to_string(), false),
        _ => (generate_token(), true),
    }
}

/// 256 random bits, hex-encoded.
pub fn generate_token() -> String {
    let mut buf = [0u8; 32];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    if !ok {
        // No /dev/urandom (non-unix): mix time, pid and addresses. Weaker, but
        // the server is loopback-only and the token is never logged elsewhere.
        let seed = format!(
            "{:?}{}{:p}",
            std::time::SystemTime::now(),
            std::process::id(),
            &buf
        );
        for (i, chunk) in buf.chunks_mut(20).enumerate() {
            let h = git2::Oid::hash_object(git2::ObjectType::Blob, format!("{seed}{i}").as_bytes())
                .map(|o| o.as_bytes().to_vec())
                .unwrap_or_default();
            for (d, s) in chunk.iter_mut().zip(h) {
                *d = s;
            }
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn serve(repo: &Path, addr: &str) -> Result<()> {
    let (token, generated) = token_from_env();
    let server = tiny_http::Server::http(addr).map_err(|e| anyhow!("cannot bind {addr}: {e}"))?;
    eprintln!(
        "gitent: serving {} on http://{}",
        repo.display(),
        server.server_addr()
    );
    if generated {
        eprintln!(
            "gitent: GITENT_TOKEN is not set; generated token (send `Authorization: Bearer <token>`):\n{token}"
        );
    } else {
        eprintln!("gitent: using the token from GITENT_TOKEN");
    }
    if let Ok(a) = addr.parse::<std::net::SocketAddr>() {
        if !a.ip().is_loopback() {
            eprintln!("gitent: warning: listening on non-loopback {a}; only requests with a loopback Host header are served");
        }
    }
    for mut req in server.incoming_requests() {
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_string(),
                    h.value.as_str().to_string(),
                )
            })
            .collect();
        let (code, v) = match authorize(&headers, &token) {
            Err(denied) => denied,
            Ok(()) => {
                let mut body = String::new();
                let _ = req.as_reader().take(MAX_BODY).read_to_string(&mut body);
                route(repo, req.method(), req.url(), &body)
            }
        };
        let mut resp =
            tiny_http::Response::from_string(serde_json::to_string_pretty(&v).unwrap_or_default())
                .with_status_code(code)
                .with_header(
                    tiny_http::Header::from_bytes("content-type", "application/json").unwrap(),
                );
        if code == 401 {
            resp = resp.with_header(
                tiny_http::Header::from_bytes("www-authenticate", "Bearer realm=\"gitent\"")
                    .unwrap(),
            );
        }
        let _ = req.respond(resp);
    }
    Ok(())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Check Host, Origin and the bearer token. `Err` carries the response.
pub fn authorize(headers: &[(String, String)], token: &str) -> Result<(), (u16, Value)> {
    let deny = |code: u16, msg: &str| Err((code, json!({"error": msg, "status": code})));
    match header(headers, "host") {
        Some(h) if is_loopback_host(host_of_authority(h)) => {}
        Some(_) => {
            return deny(
                403,
                "forbidden: Host must be a loopback address (localhost, 127.0.0.1, [::1])",
            )
        }
        None => return deny(403, "forbidden: missing Host header"),
    }
    if let Some(o) = header(headers, "origin") {
        let host = o
            .split_once("://")
            .map(|(_, rest)| host_of_authority(rest.split('/').next().unwrap_or("")))
            .unwrap_or("");
        if !is_loopback_host(host) {
            return deny(403, "forbidden: cross-origin requests are not accepted");
        }
    }
    let presented = header(headers, "authorization")
        .and_then(|a| {
            let (scheme, rest) = a.trim().split_once(' ')?;
            scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
        })
        .unwrap_or("");
    if presented.is_empty() {
        return deny(
            401,
            "unauthorized: send `Authorization: Bearer <token>` (GITENT_TOKEN)",
        );
    }
    if !constant_time_eq(presented.as_bytes(), token.as_bytes()) {
        return deny(401, "unauthorized: invalid token");
    }
    Ok(())
}

/// `host[:port]` or `[v6]:port` → host.
fn host_of_authority(a: &str) -> &str {
    let a = a.trim();
    if let Some(rest) = a.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    match a.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => a,
    }
}

pub fn is_loopback_host(h: &str) -> bool {
    let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }
    match h.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn route(repo: &Path, method: &tiny_http::Method, url: &str, body: &str) -> (u16, Value) {
    use tiny_http::Method::*;
    let path = url.split('?').next().unwrap_or("");
    match (method, path) {
        (Get, "/v1/tools") => (
            200,
            json!({"schema": api::TOOLS_SCHEMA, "tools": api::tool_defs()}),
        ),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn loopback_hosts() {
        for ok in [
            "localhost",
            "127.0.0.1",
            "127.1.2.3",
            "::1",
            "LOCALHOST.",
            "a.localhost",
        ] {
            assert!(is_loopback_host(ok), "{ok}");
        }
        for bad in ["evil.com", "10.0.0.1", "0.0.0.0", "localhost.evil.com", ""] {
            assert!(!is_loopback_host(bad), "{bad}");
        }
        assert_eq!(host_of_authority("localhost:7341"), "localhost");
        assert_eq!(host_of_authority("[::1]:7341"), "::1");
        assert_eq!(host_of_authority("::1"), "::1");
    }

    #[test]
    fn authorization_rules() {
        let t = "s3cret";
        let good = [
            ("Host", "127.0.0.1:7341"),
            ("Authorization", "Bearer s3cret"),
        ];
        assert!(authorize(&h(&good), t).is_ok());
        assert!(authorize(
            &h(&[("host", "localhost"), ("authorization", "bearer s3cret")]),
            t
        )
        .is_ok());
        assert!(authorize(
            &h(&[
                ("Host", "localhost"),
                ("Origin", "http://localhost:3000"),
                ("Authorization", "Bearer s3cret")
            ]),
            t
        )
        .is_ok());
        let code = |hs: &[(&str, &str)]| authorize(&h(hs), t).unwrap_err().0;
        assert_eq!(code(&[("Host", "localhost")]), 401);
        assert_eq!(
            code(&[("Host", "localhost"), ("Authorization", "Bearer nope")]),
            401
        );
        assert_eq!(
            code(&[("Host", "localhost"), ("Authorization", "Basic s3cret")]),
            401
        );
        assert_eq!(
            code(&[("Host", "evil.com"), ("Authorization", "Bearer s3cret")]),
            403
        );
        assert_eq!(code(&[("Authorization", "Bearer s3cret")]), 403);
        assert_eq!(
            code(&[
                ("Host", "localhost"),
                ("Origin", "https://evil.com"),
                ("Authorization", "Bearer s3cret")
            ]),
            403
        );
        assert_eq!(
            code(&[
                ("Host", "localhost"),
                ("Origin", "null"),
                ("Authorization", "Bearer s3cret")
            ]),
            403
        );
    }

    #[test]
    fn generated_tokens_are_random_hex() {
        let (a, b) = (generate_token(), generate_token());
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
