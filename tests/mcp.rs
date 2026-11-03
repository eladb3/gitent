//! Criteria E3/E4 and C8 over MCP: agents drive gitent through MCP (stdio) and HTTP.
mod common;
use common::*;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};

struct Mcp {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    out: BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Mcp {
    fn start(r: &TestRepo) -> Mcp {
        let mut child = Command::new(bin())
            .arg("mcp")
            .current_dir(&r.path)
            .env_remove("GITENT_EXECUTOR")
            .env("PYTHONDONTWRITEBYTECODE", "1")
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

    let unknown = m.request("bogus/method", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);

    // Agents default to host mode: Gitent prepares, the agent edits, Gitent validates.
    let res = m.call("gitent_cherry_pick", json!({"commit": feature}));
    assert_eq!(res["isError"], false, "{res}");
    let op = &res["structuredContent"];
    assert_eq!(op["state"], "awaiting_host");
    let wt = std::path::PathBuf::from(op["worktree"].as_str().unwrap());
    std::fs::write(wt.join("app/accounts.py"), ACCOUNTS_CACHED_PY).unwrap();
    std::fs::write(wt.join("tests/test_cache.py"), TEST_CACHE_ACCOUNTS_PY).unwrap();
    let res = m.call("gitent_continue", json!({}));
    assert_eq!(res["structuredContent"]["state"], "succeeded", "{res}");
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

fn http(port: u16, method: &str, path: &str, body: &str) -> (u16, Value) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let code: u16 = resp.split_whitespace().nth(1).unwrap().parse().unwrap();
    let json_body = resp.split("\r\n\r\n").nth(1).unwrap_or("{}");
    (code, serde_json::from_str(json_body).unwrap_or(Value::Null))
}

#[test]
fn http_api() {
    let r = TestRepo::users();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let mut child = Command::new(bin())
        .args(["serve", "--addr", &format!("127.0.0.1:{port}")])
        .current_dir(&r.path)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut up = false;
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(up, "server did not start");
    let (code, v) = http(port, "GET", "/v1/tools", "");
    assert_eq!(code, 200);
    assert!(v["tools"].as_array().unwrap().len() >= 10);
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
    let _ = child.kill();
    let _ = child.wait();
}
