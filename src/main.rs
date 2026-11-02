use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use gitent::api::{self, Surface};
use gitent::config::Config;
use gitent::repo::{short, Repo};
use serde_json::{json, Value};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "gitent",
    version,
    about = "Agent-native version control: semantic intentions on top of Git"
)]
struct Cli {
    /// Run as if started in <path>
    #[arg(short = 'C', global = true, default_value = ".")]
    path: PathBuf,
    /// Print exactly one JSON document on stdout
    #[arg(long, global = true)]
    json: bool,
    /// Executor for this command: builtin | command | host
    #[arg(long, global = true)]
    executor: Option<String>,
    /// Command line for the command executor
    #[arg(long, global = true)]
    executor_command: Option<String>,
    /// LLM provider: anthropic | openai | command | scripted | none
    #[arg(long, global = true)]
    provider: Option<String>,
    /// Override validation.test_command
    #[arg(long, global = true)]
    test_command: Option<String>,
    /// Override validation.max_attempts
    #[arg(long, global = true)]
    max_attempts: Option<u32>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Initialize gitent in a (new or existing) git repository
    Init {
        /// Preset: claude-code | codex | kiro | gemini | aider | builtin | openai | host
        #[arg(long)]
        agent: Option<String>,
        /// Write to .git/gitent/config.toml (local, not shared) instead of .gitent/config.toml
        #[arg(long)]
        local: bool,
        /// Overwrite an existing config
        #[arg(long)]
        force: bool,
    },
    /// Record a semantic commit of all changes (or --staged)
    Commit {
        #[arg(short, long)]
        message: Option<String>,
        /// JSON file with an intent draft (see `gitent schema`)
        #[arg(long)]
        intent_file: Option<PathBuf>,
        #[arg(long)]
        staged: bool,
        /// Fail instead of falling back to heuristic analysis
        #[arg(long)]
        require_llm: bool,
        /// Analyze only; do not commit
        #[arg(long)]
        dry_run: bool,
    },
    /// Semantic diff: analyze uncommitted changes, or list intentions between revisions
    Diff {
        from: Option<String>,
        to: Option<String>,
    },
    /// History of intentions
    Log {
        rev: Option<String>,
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
    },
    /// Show the semantic record of a commit or intent id
    Show { rev: Option<String> },
    /// Re-implement commits' intentions on the current branch
    CherryPick {
        #[arg(required = true)]
        commits: Vec<String>,
        #[command(flatten)]
        o: OpFlags,
    },
    /// Merge a branch by composing intentions
    Merge {
        branch: String,
        /// Proceed even if intentions look incompatible
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        o: OpFlags,
    },
    /// Replay this branch's intentions onto another base
    Rebase {
        onto: String,
        #[command(flatten)]
        o: OpFlags,
    },
    /// Remove a commit's intention from the current branch
    Revert {
        commit: String,
        #[command(flatten)]
        o: OpFlags,
    },
    /// Continue an operation awaiting the host agent
    Continue { op_id: Option<String> },
    /// Abort an operation (removes its worktree; branch is untouched)
    Abort { op_id: Option<String> },
    /// Repository and operation status
    Status,
    /// List operations
    Ops,
    /// Create semantic records for commits made with plain git
    Annotate {
        rev: Option<String>,
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
    },
    /// Serve the tool API over MCP (stdio)
    Mcp,
    /// Serve the tool API over HTTP
    Serve {
        #[arg(long, default_value = "127.0.0.1:7341")]
        addr: String,
    },
    /// Print the JSON schema of semantic records
    Schema,
}

#[derive(clap::Args, Default)]
struct OpFlags {
    /// Refuse to finalize if validation produced warnings
    #[arg(long)]
    strict: bool,
    /// Skip the deterministic git attempt
    #[arg(long)]
    no_git: bool,
}

fn main() {
    let cli = Cli::parse();
    let json_out = cli.json;
    match run(cli) {
        Ok((v, code)) => {
            if json_out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            std::process::exit(code);
        }
        Err(e) => {
            if json_out {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({"error": format!("{e:#}")})).unwrap()
                );
            } else {
                eprintln!("gitent: error: {e:#}");
            }
            std::process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<(Value, i32)> {
    let mut args = json!({});
    for (k, v) in [
        ("executor", &cli.executor),
        ("executor_command", &cli.executor_command),
        ("provider", &cli.provider),
        ("test_command", &cli.test_command),
    ] {
        if let Some(v) = v {
            args[k] = json!(v);
        }
    }
    if let Some(m) = cli.max_attempts {
        args["max_attempts"] = json!(m);
    }
    let path = cli.path.clone();
    let human = !cli.json;
    let op_flags = |a: &mut Value, o: &OpFlags| {
        a["strict"] = json!(o.strict);
        a["no_git"] = json!(o.no_git);
    };
    let (tool, renderer): (&str, fn(&Value)) = match &cli.cmd {
        Cmd::Init {
            agent,
            local,
            force,
        } => {
            let v = init(&path, agent.as_deref(), *local, *force)?;
            if human {
                println!("{}", v["message"].as_str().unwrap_or(""));
            }
            return Ok((v, 0));
        }
        Cmd::Mcp => {
            gitent::mcp::serve_stdio(&path)?;
            return Ok((json!({}), 0));
        }
        Cmd::Serve { addr } => {
            gitent::http::serve(&path, addr)?;
            return Ok((json!({}), 0));
        }
        Cmd::Commit {
            message,
            intent_file,
            staged,
            require_llm,
            dry_run,
        } => {
            if let Some(m) = message {
                args["message"] = json!(m);
            }
            if let Some(f) = intent_file {
                let text = std::fs::read_to_string(f)
                    .with_context(|| format!("reading {}", f.display()))?;
                args["intent"] =
                    serde_json::from_str(&text).context("intent file is not valid JSON")?;
            }
            args["staged_only"] = json!(staged);
            args["require_llm"] = json!(require_llm);
            args["dry_run"] = json!(dry_run);
            ("commit", render_commit)
        }
        Cmd::Diff { from, to } => {
            if let Some(f) = from {
                args["from"] = json!(f);
            }
            if let Some(t) = to {
                args["to"] = json!(t);
            }
            ("diff", render_diff)
        }
        Cmd::Log { rev, limit } => {
            if let Some(r) = rev {
                args["rev"] = json!(r);
            }
            args["limit"] = json!(limit);
            ("log", render_log)
        }
        Cmd::Show { rev } => {
            args["rev"] = json!(rev.clone().unwrap_or_else(|| "HEAD".into()));
            ("show", render_show)
        }
        Cmd::CherryPick { commits, o } => {
            args["commits"] = json!(commits);
            op_flags(&mut args, o);
            ("cherry_pick", render_op)
        }
        Cmd::Merge { branch, force, o } => {
            args["branch"] = json!(branch);
            args["force"] = json!(force);
            op_flags(&mut args, o);
            ("merge", render_op)
        }
        Cmd::Rebase { onto, o } => {
            args["onto"] = json!(onto);
            op_flags(&mut args, o);
            ("rebase", render_op)
        }
        Cmd::Revert { commit, o } => {
            args["commit"] = json!(commit);
            op_flags(&mut args, o);
            ("revert", render_op)
        }
        Cmd::Continue { op_id } => {
            if let Some(i) = op_id {
                args["op_id"] = json!(i);
            }
            ("continue", render_op)
        }
        Cmd::Abort { op_id } => {
            if let Some(i) = op_id {
                args["op_id"] = json!(i);
            }
            ("abort", render_op)
        }
        Cmd::Status => ("status", render_status),
        Cmd::Ops => ("ops", render_ops),
        Cmd::Annotate { rev, limit } => {
            if let Some(r) = rev {
                args["rev"] = json!(r);
            }
            args["limit"] = json!(limit);
            ("annotate", |v| {
                println!("{}", serde_json::to_string_pretty(v).unwrap())
            })
        }
        Cmd::Schema => ("schema", |v| {
            println!("{}", serde_json::to_string_pretty(v).unwrap())
        }),
    };
    let v = api::call(&path, tool, &args, Surface::Cli)?;
    if human {
        renderer(&v);
    }
    let code = if gitent::mcp::is_failed(&v) { 2 } else { 0 };
    Ok((v, code))
}

fn init(path: &std::path::Path, agent: Option<&str>, local: bool, force: bool) -> Result<Value> {
    let repo = Repo::init(path)?;
    let target = if local {
        repo.gitent_dir().join("config.toml")
    } else {
        repo.workdir.join(".gitent/config.toml")
    };
    std::fs::create_dir_all(repo.gitent_dir())?;
    let preset = agent.unwrap_or("host");
    let mut cfg = match Config::preset(preset) {
        Some(c) => c,
        None => bail!(
            "unknown agent preset '{preset}' (one of: {})",
            Config::PRESETS.join(", ")
        ),
    };
    if target.exists() && !force {
        return Ok(
            json!({"initialized": false, "config": target, "message": format!(
            "gitent already initialized ({} exists; use --force to overwrite)", target.display())}),
        );
    }
    if cfg.validation.test_command.is_empty() {
        cfg.validation.test_command = detect_test_command(&repo.workdir);
    }
    std::fs::create_dir_all(target.parent().unwrap())?;
    let header = format!(
        "# Gitent configuration (preset: {preset}). See docs/INTEGRATIONS.md.\n\
         # Local overrides: .git/gitent/config.toml. Env overrides: GITENT_EXECUTOR, GITENT_PROVIDER, ...\n\n"
    );
    std::fs::write(&target, header + &cfg.to_toml())?;
    Ok(
        json!({"initialized": true, "config": target, "preset": preset, "message": format!(
        "Initialized gitent in {} (preset: {preset}, config: {}){}",
        repo.workdir.display(), target.display(),
        if cfg.validation.test_command.is_empty() { "\nhint: set validation.test_command so operations can be validated".to_string() } else { format!("\ntest command: {}", cfg.validation.test_command) })}),
    )
}

fn detect_test_command(dir: &std::path::Path) -> String {
    if dir.join("Cargo.toml").exists() {
        "cargo test".into()
    } else if dir.join("package.json").exists() {
        "npm test --silent".into()
    } else if dir.join("go.mod").exists() {
        "go test ./...".into()
    } else if dir.join("pyproject.toml").exists()
        || dir.join("setup.py").exists()
        || dir.join("tests").is_dir()
    {
        "python3 -m unittest discover -q".into()
    } else {
        String::new()
    }
}

// ---------- human renderers ----------

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

fn render_record(r: &Value) {
    println!(
        "intent   {}  [{}]  {}",
        s(r, "intent_id"),
        s(r, "kind"),
        s(r, "title")
    );
    if !s(r, "intent").is_empty() {
        println!("why      {}", s(r, "intent"));
    }
    if !s(r, "behavior").is_empty() {
        println!("behavior {}", s(r, "behavior"));
    }
    for (label, key) in [
        ("must", "acceptance"),
        ("keep", "constraints"),
        ("tests", "tests"),
    ] {
        if let Some(a) = r.get(key).and_then(|a| a.as_array()) {
            for x in a {
                println!("{label:<8} {}", x.as_str().unwrap_or(""));
            }
        }
    }
    if let Some(sym) = r["affected"]["symbols"].as_array() {
        let names: Vec<String> = sym
            .iter()
            .map(|x| {
                format!(
                    "{}{}",
                    s(x, "name"),
                    if x["verified"] == false { "?" } else { "" }
                )
            })
            .collect();
        if !names.is_empty() {
            println!("symbols  {}", names.join(", "));
        }
    }
    let p = &r["provenance"];
    println!(
        "origin   {} via {}{} (analysis: {}, confidence {:.2})",
        s(p, "operation"),
        if s(p, "executor").is_empty() {
            "author"
        } else {
            s(p, "executor")
        },
        p.get("derived_from")
            .map(|d| format!(" from {}", short(s(d, "commit"))))
            .unwrap_or_default(),
        s(&p["analysis"], "method"),
        r["confidence"].as_f64().unwrap_or(0.0)
    );
    if let Some(w) = p["analysis"]["warnings"].as_array() {
        for x in w {
            println!("warning  {}", x.as_str().unwrap_or(""));
        }
    }
}

fn render_commit(v: &Value) {
    match v["commit"].as_str() {
        Some(c) => println!("[{}] {}", short(c), s(&v["record"], "title")),
        None => println!("(dry run — not committed)"),
    }
    render_record(&v["record"]);
}

fn render_diff(v: &Value) {
    if let Some(intents) = v["intents"].as_array() {
        println!(
            "{} intention(s) in {}..{}",
            intents.len(),
            short(s(v, "from")),
            short(s(v, "to"))
        );
        for e in intents {
            print_log_line(e);
        }
        return;
    }
    if let Some(files) = v["files"].as_array() {
        for f in files {
            println!("  {:<9} {}", s(f, "status"), s(f, "path"));
        }
    }
    println!();
    render_record(&v["record"]);
}

fn print_log_line(e: &Value) {
    let tag = if e.get("intent_id").is_some() {
        let mut t = format!("{} [{}]", s(e, "intent_id"), s(e, "kind"));
        if let Some(d) = e.get("derived_from") {
            t.push_str(&format!(
                " ← {} via {}",
                short(s(d, "commit")),
                s(e, "executor")
            ));
        }
        if let Some(d) = e.get("reverts") {
            t.push_str(&format!(" reverts {}", s(d, "intent_id")));
        }
        if e["validated"] == true {
            t.push_str(" ✓");
        }
        t
    } else {
        "(no semantic record)".into()
    };
    let title = if s(e, "title").is_empty() {
        s(e, "summary")
    } else {
        s(e, "title")
    };
    println!("{}  {}  {}", s(e, "short"), title, tag);
}

fn render_log(v: &Value) {
    for e in v["entries"].as_array().into_iter().flatten() {
        print_log_line(e);
    }
}

fn render_show(v: &Value) {
    println!("commit   {}", s(v, "commit"));
    println!("{}", s(v, "message").trim_end());
    println!();
    if v["record"].is_null() {
        println!("(no semantic record — run `gitent annotate`)");
    } else {
        render_record(&v["record"]);
        if let Some(rz) = v["realizations"].as_array() {
            if rz.len() > 1 {
                println!(
                    "realized {}",
                    rz.iter()
                        .map(|x| short(x.as_str().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
}

fn render_op(v: &Value) {
    let state = s(v, "state");
    match state {
        "up_to_date" | "fast_forward" => {
            println!(
                "{}",
                if state == "fast_forward" {
                    format!("fast-forwarded to {}", short(s(v, "result_commit")))
                } else {
                    s(v, "message").to_string()
                }
            );
            return;
        }
        _ => {}
    }
    println!("{} {} — {}", s(v, "kind"), s(v, "id"), state);
    for (i, st) in v["steps"].as_array().into_iter().flatten().enumerate() {
        let k = &st["kind"];
        let what = k
            .get("source")
            .or(k.get("theirs"))
            .and_then(|x| x.as_str())
            .map(short)
            .unwrap_or_default();
        println!(
            "  step {} {} {} → {}",
            i + 1,
            s(k, "type"),
            what,
            s(st, "status")
        );
        for a in st["attempts"].as_array().into_iter().flatten() {
            let val = &a["validation"];
            let mark = if val["passed"] == true {
                "pass"
            } else {
                "fail"
            };
            println!("    attempt {} by {}: {}", a["n"], s(a, "by"), mark);
            for c in val["checks"].as_array().into_iter().flatten() {
                if c["passed"] == false {
                    let d = s(c, "detail");
                    let first = d.lines().take(6).collect::<Vec<_>>().join("\n        ");
                    println!("      ✗ {}: {}", s(c, "name"), first);
                }
            }
        }
        if let Some(c) = st["result_commit"].as_str() {
            println!("    → {}", short(c));
        }
    }
    for w in v["warnings"].as_array().into_iter().flatten() {
        println!("  warning: {}", w.as_str().unwrap_or(""));
    }
    match state {
        "succeeded" => println!(
            "✓ {} updated to {}",
            s(v, "branch_ref").trim_start_matches("refs/heads/"),
            short(s(v, "result_commit"))
        ),
        "awaiting_host" => {
            println!("brief:    {}", s(v, "brief"));
            println!("worktree: {}", s(v, "worktree"));
            println!("{}", s(v, "next"));
        }
        "failed" => println!("✗ {}", s(v, "error")),
        _ => {}
    }
}

fn render_status(v: &Value) {
    println!("branch   {}", s(v, "branch"));
    println!("head     {}", short(s(v, "head")));
    if let Some(h) = v["head_intent"].as_object() {
        println!(
            "intent   {} {}",
            h["intent_id"].as_str().unwrap_or(""),
            h["title"].as_str().unwrap_or("")
        );
    }
    let dirty = v["dirty"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("dirty    {dirty} file(s)");
    let c = &v["config"];
    println!("llm      {}", s(c, "llm_provider"));
    println!("executor {} {}", s(c, "executor"), s(c, "executor_command"));
    println!(
        "tests    {}",
        if s(c, "test_command").is_empty() {
            "(none configured)"
        } else {
            s(c, "test_command")
        }
    );
    for o in v["open_operations"].as_array().into_iter().flatten() {
        println!("open op  {} {} {}", s(o, "id"), s(o, "kind"), s(o, "state"));
    }
}

fn render_ops(v: &Value) {
    for o in v["operations"].as_array().into_iter().flatten() {
        println!(
            "{}  {:<11} {:<13} {}",
            s(o, "id"),
            s(o, "kind"),
            s(o, "state"),
            s(o, "result_commit")
        );
    }
}
