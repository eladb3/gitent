//! Criteria E3/E4 and C8 over MCP: agents drive gitent through MCP (stdio) and HTTP;
//! criteria S1-S4: the tool surfaces' trust boundaries (ADR 0014).
mod common;
use common::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Stdio;

struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    out: BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Mcp {
    fn start(r: &TestRepo) -> Mcp {
        let mut child = r
            .gitent_cmd()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Mcp {
            child,
            stdin,
            out,
            id: 0,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.id);
        v
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let v = self.request("tools/call", json!({"name": tool, "arguments": args}));
        v["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn mcp_stdio_end_to_end() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    r.rename_on_current();
    let mut m = Mcp::start(&r);

    let init = m.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "gitent");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    // notifications get no response; the next request still lines up
    writeln!(
        m.stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();

    let tools = m.request("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in [
        "gitent_commit",
        "gitent_cherry_pick",
        "gitent_continue",
        "gitent_check",
        "gitent_merge",
        "gitent_revert",
    ] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    assert!(tools["result"]["tools"][0]["inputSchema"]["type"] == "object");
    assert_eq!(tools["result"]["_meta"]["gitent/schema"], "gitent/tools@2");

    let unknown = m.request("bogus/method", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);

    // Agents default to host mode: Gitent prepares, the agent edits, Gitent validates.
    let res = m.call("gitent_cherry_pick", json!({"commit": feature}));
    assert_eq!(res["isError"], false, "{res}");
    let op = &res["structuredContent"];
    assert_eq!(op["state"], "awaiting_host");
    let wt = std::path::PathBuf::from(op["worktree"].as_str().unwrap());
    // S6: worktrees live in $TMPDIR/gitent-<uid>/<repo-hash>/, private to the user.
    let user_dir = wt.parent().unwrap().parent().unwrap();
    assert_eq!(user_dir.parent().unwrap(), r.tmp.as_path(), "{wt:?}");
    let uid = std::process::Command::new("id").arg("-u").output().unwrap();
    assert_eq!(
        user_dir.file_name().unwrap().to_string_lossy(),
        format!("gitent-{}", String::from_utf8_lossy(&uid.stdout).trim())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for d in [user_dir, wt.parent().unwrap()] {
            let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{d:?}");
        }
    }
    std::fs::write(wt.join("app/accounts.py"), ACCOUNTS_CACHED_PY).unwrap();
    std::fs::write(wt.join("tests/test_cache.py"), TEST_CACHE_ACCOUNTS_PY).unwrap();
    let res = m.call("gitent_continue", json!({}));
    assert_eq!(res["structuredContent"]["state"], "succeeded", "{res}");
    assert!(
        !wt.exists() && !wt.parent().unwrap().exists(),
        "worktree and its empty per-repo dir are removed"
    );
    // V6: verify over MCP (the cherry-pick just made is a valid move of `feature`).
    let res = m.call("gitent_verify", json!({"source": feature}));
    assert_eq!(res["structuredContent"]["verified"], true, "{res}");
    let res = m.call(
        "gitent_verify",
        json!({"source": feature, "result": "HEAD~1", "onto": "HEAD~2"}),
    );
    assert_eq!(
        res["isError"], true,
        "the rename commit is not a move of the caching change: {res}"
    );

    // The host agent commits with its own intent and requirements.
    r.write(
        "app/db.py",
        &DB_PY.replace("CALLS = {\"n\": 0}", "CALLS = {\"n\": 0}\nMAX_USERS = 1000"),
    );
    let res = m.call("gitent_commit", json!({
        "message": "Add a user limit constant",
        "intent": {"title": "Introduce MAX_USERS", "intent": "Prepare for capacity limits", "behavior": "app.db.MAX_USERS == 1000"},
        "requirements": [{"statement": "MAX_USERS is 1000", "check": {"run": "python3 -c 'import app.db as d; assert d.MAX_USERS == 1000'", "expect_exit": 0}}]
    }));
    assert_eq!(res["isError"], false, "{res}");
    let rec = &res["structuredContent"]["record"];
    assert_eq!(rec["provenance"]["analysis"]["method"], "host");
    assert_eq!(rec["requirements"][0]["proof"]["before"], "fail");
    assert_eq!(rec["requirements"][0]["proof"]["after"], "pass");
    // S3: a command check supplied by an agent is never stored as the author's.
    assert_eq!(rec["requirements"][0]["origin"], "agent", "{rec}");
    assert_eq!(
        r.note("HEAD")["requirements"][0]["origin"],
        "agent",
        "stored note"
    );

    let res = m.call("gitent_check", json!({}));
    assert_eq!(res["structuredContent"]["passed"], true, "{res}");
    assert_eq!(res["structuredContent"]["counts"]["pass"], 2);

    let res = m.call("gitent_log", json!({"limit": 3}));
    let entries = res["structuredContent"]["entries"].as_array().unwrap();
    assert_eq!(entries[0]["title"], "Introduce MAX_USERS");
    assert_eq!(entries[1]["derived_from"]["commit"], feature.as_str());

    let res = m.call("gitent_show", json!({"rev": "nope"}));
    assert_eq!(res["isError"], true);
}

const TOKEN: &str = "test-token-0123456789";

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, Value) {
    let auth = format!("Authorization: Bearer {TOKEN}\r\n");
    http_raw(
        port,
        method,
        path,
        body,
        &format!("Host: localhost\r\n{auth}"),
    )
}

/// A request with exactly the given header lines (each ending in \r\n).
fn http_raw(port: u16, method: &str, path: &str, body: &str, headers: &str) -> (u16, Value) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let code: u16 = resp.split_whitespace().nth(1).unwrap().parse().unwrap();
    let json_body = resp.split("\r\n\r\n").nth(1).unwrap_or("{}");
    (code, serde_json::from_str(json_body).unwrap_or(Value::Null))
}

struct Server {
    child: std::process::Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `gitent serve` on a free port; `token` = GITENT_TOKEN (None: let it generate one).
fn serve(r: &TestRepo, token: Option<&str>) -> (Server, std::process::ChildStderr) {
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut c = r.gitent_cmd();
    c.args(["serve", "--addr", &format!("127.0.0.1:{port}")])
        .stderr(Stdio::piped());
    if let Some(t) = token {
        c.env("GITENT_TOKEN", t);
    }
    let mut child = c.spawn().unwrap();
    let stderr = child.stderr.take().unwrap();
    let mut up = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(up, "server did not start");
    (Server { child, port }, stderr)
}

#[test]
fn http_api() {
    let r = TestRepo::users();
    let (srv, _stderr) = serve(&r, Some(TOKEN));
    let port = srv.port;
    let (code, v) = http(port, "GET", "/v1/tools", "");
    assert_eq!(code, 200);
    assert!(v["tools"].as_array().unwrap().len() >= 10);
    assert_eq!(v["schema"], "gitent/tools@2");
    let (code, v) = http(port, "POST", "/v1/tools/gitent_status", "{}");
    assert_eq!(code, 200);
    assert_eq!(v["branch"], "main");
    let (code, v) = http(port, "POST", "/v1/tools/gitent_show", r#"{"rev": "zzz"}"#);
    assert_eq!(code, 400);
    assert!(v["error"].is_string());
    let (code, v) = http(
        port,
        "POST",
        "/mcp",
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
    );
    assert_eq!(code, 200);
    assert_eq!(v["id"], 7);
    let (code, _) = http(port, "GET", "/nope", "");
    assert_eq!(code, 404);
}

/// S1: no token, a wrong token, a foreign Host (DNS rebinding) or a foreign
/// Origin (CSRF) are all refused before any tool runs.
#[test]
fn http_refuses_unauthenticated_rebinding_and_cross_origin_requests() {
    let r = TestRepo::users();
    let (srv, _stderr) = serve(&r, Some(TOKEN));
    let port = srv.port;
    let auth = format!("Authorization: Bearer {TOKEN}\r\n");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    for (headers, want, why) in [
        ("Host: localhost\r\n".to_string(), 401, "no token"),
        (
            "Host: localhost\r\nAuthorization: Bearer wrong\r\n".to_string(),
            401,
            "bad token",
        ),
        (
            format!("Host: evil.example:{port}\r\n{auth}"),
            403,
            "rebinding Host",
        ),
        (auth.clone(), 403, "missing Host"),
        (
            format!("Host: localhost\r\nOrigin: https://evil.example\r\n{auth}"),
            403,
            "evil Origin",
        ),
        (
            format!("Host: localhost\r\nOrigin: null\r\n{auth}"),
            403,
            "opaque Origin",
        ),
    ] {
        for (m, p, b) in [
            ("GET", "/v1/tools", ""),
            ("POST", "/v1/tools/gitent_status", "{}"),
            ("POST", "/mcp", body),
        ] {
            let (code, v) = http_raw(port, m, p, b, &headers);
            assert_eq!(code, want, "{why}: {m} {p} -> {v}");
            assert!(
                v["error"].is_string(),
                "{why}: JSON error expected, got {v}"
            );
            assert!(v.get("tools").is_none() && v.get("result").is_none());
        }
    }
    // Same-origin browser requests from a loopback page are fine.
    let (code, _) = http_raw(
        port,
        "GET",
        "/v1/tools",
        "",
        &format!("Host: 127.0.0.1:{port}\r\nOrigin: http://localhost:{port}\r\n{auth}"),
    );
    assert_eq!(code, 200);
}

/// S1: without GITENT_TOKEN, a random token is generated and printed once.
#[test]
fn http_generates_a_token_when_none_is_configured() {
    let r = TestRepo::users();
    let (srv, stderr) = serve(&r, None);
    let mut lines = BufReader::new(stderr).lines();
    let mut token = String::new();
    for _ in 0..10 {
        let l = lines.next().unwrap().unwrap();
        let t = l.trim();
        if t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
            token = t.to_string();
            break;
        }
    }
    assert!(!token.is_empty(), "no generated token on stderr");
    let (code, _) = http(srv.port, "GET", "/v1/tools", "");
    assert_eq!(code, 401, "the test token must not work");
    let (code, v) = http_raw(
        srv.port,
        "POST",
        "/v1/tools/gitent_status",
        "{}",
        &format!("Host: localhost\r\nAuthorization: Bearer {token}\r\n"),
    );
    assert_eq!(code, 200, "{v}");
}

/// S2: MCP/HTTP tool arguments cannot choose commands; the CLI still can.
#[test]
fn tool_arguments_cannot_choose_commands() {
    let r = TestRepo::users();
    let feature = r.add_cache_feature();
    let marker = r.tools.join("pwned");
    let evil = format!("touch {}", marker.display());
    let mut m = Mcp::start(&r);
    let tools = m.request("tools/list", json!({}));
    let text = tools["result"].to_string();
    for k in [
        "executor_command",
        "test_command",
        "test_selector",
        "llm_command",
    ] {
        assert!(!text.contains(k), "{k} advertised in tool schemas");
    }
    for (tool, args) in [
        (
            "gitent_cherry_pick",
            json!({"commit": feature, "executor": "command", "executor_command": evil}),
        ),
        ("gitent_check", json!({"test_command": evil})),
        (
            "gitent_verify",
            json!({"source": feature, "test_selector": evil}),
        ),
        ("gitent_status", json!({"provider": "command"})),
    ] {
        let res = m.call(tool, args);
        assert_eq!(res["isError"], true, "{tool}: {res}");
        assert!(
            res["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("cannot be set through the tool API"),
            "{res}"
        );
    }
    assert!(!marker.exists(), "a tool argument ran a command");
    // The same over HTTP.
    let (srv, _e) = serve(&r, Some(TOKEN));
    let (code, v) = http(
        srv.port,
        "POST",
        "/v1/tools/gitent_check",
        &json!({"test_command": evil}).to_string(),
    );
    assert_eq!(code, 400, "{v}");
    assert!(!marker.exists());
    // The CLI keeps its flags.
    let (code, v) = r.gitent(&["--test-command", &evil, "status"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["config"]["test_command"], evil.as_str());
}

/// S4: crafted operation ids are refused and never touch the filesystem.
#[test]
fn crafted_op_ids_are_refused() {
    let r = TestRepo::users();
    let victim = r.tools.join("victim");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("keep.txt"), "x").unwrap();
    let crafted = format!("op_x/../../../../../../../..{}", victim.display());
    let mut m = Mcp::start(&r);
    for tool in ["gitent_abort", "gitent_continue"] {
        for id in [crafted.as_str(), "../op_x", "op_", "op_a b"] {
            let res = m.call(tool, json!({"op_id": id}));
            assert_eq!(res["isError"], true, "{tool} {id}: {res}");
            assert!(
                res["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("invalid operation id"),
                "{res}"
            );
        }
    }
    let (code, v) = r.gitent(&["abort", &crafted]);
    assert_ne!(code, 0);
    assert!(
        v["error"]
            .as_str()
            .unwrap()
            .contains("invalid operation id"),
        "{v}"
    );
    assert!(victim.join("keep.txt").exists());
}
