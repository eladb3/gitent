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
    #[arg(long, global = true, help_heading = "Overrides")]
    executor: Option<String>,
    /// Command line for the command executor
    #[arg(long, global = true, help_heading = "Overrides")]
    executor_command: Option<String>,
    /// LLM provider: anthropic | openai | command | scripted | none
    #[arg(long, global = true, help_heading = "Overrides")]
    provider: Option<String>,
    /// Override validation.test_command
    #[arg(long, global = true, help_heading = "Overrides")]
    test_command: Option<String>,
    /// Override validation.test_selector (`{test}`, `{test_re}`, `{test_esc}`, `{file}`, `{module}` are replaced, shell-quoted)
    #[arg(long, global = true, help_heading = "Overrides")]
    test_selector: Option<String>,
    /// Override validation.max_attempts
    #[arg(long, global = true, help_heading = "Overrides")]
    max_attempts: Option<u32>,
    /// Run `command` requirements written by an agent or LLM (origin agent/llm); review them first
    #[arg(long, global = true, help_heading = "Overrides")]
    trust_agent_commands: bool,
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
    /// Allow this repository's shared config (.gitent/config.toml) to set
    /// executor.command, llm.command and llm.base_url (stores its sha256 in
    /// .git/gitent/trusted; any change to the file needs trusting again)
    Trust,
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
        /// A requirement this change satisfies (repeatable). Follow it with --test or --check to make it executable.
        #[arg(long, action = clap::ArgAction::Append)]
        require: Vec<String>,
        /// Test name that checks the preceding --require
        #[arg(long, action = clap::ArgAction::Append)]
        test: Vec<String>,
        /// Shell command (exit 0 = pass) that checks the preceding --require
        #[arg(long, action = clap::ArgAction::Append)]
        check: Vec<String>,
        /// Retire a live requirement: --retire <req_id> "<reason>"
        #[arg(long, num_args = 2, value_names = ["REQ_ID", "REASON"], action = clap::ArgAction::Append)]
        retire: Vec<String>,
        /// Reject the commit if it breaks any live requirement
        #[arg(long)]
        check_ledger: bool,
        /// Skip proving requirements (fail-before / pass-after)
        #[arg(long)]
        no_prove: bool,
        /// Allow this requirement to fail after the change (repeatable)
        #[arg(long, action = clap::ArgAction::Append)]
        allow_failing: Vec<String>,
    },
    /// Verify that an existing result correctly moves a change (any agent, bot or human made it)
    Verify {
        /// The change that was moved (commit or int_…); for --kind merge, the merged branch
        source: String,
        /// Commit to verify (default HEAD)
        result: Option<String>,
        /// The target before the change (default: first parent of the result)
        #[arg(long)]
        onto: Option<String>,
        /// Verify uncommitted changes against HEAD (your files and index are not modified)
        #[arg(long)]
        working_tree: bool,
        /// apply | revert | merge
        #[arg(long, default_value = "apply")]
        kind: String,
        /// If verified, attach a semantic record (provenance + requirements) to the result
        #[arg(long)]
        record: bool,
        /// Skip the LLM verifier even if configured
        #[arg(long)]
        no_llm: bool,
    },
    /// Verify the backports/cherry-picks/reverts in base..head (for PR checks; see docs/GITHUB_ACTION.md)
    VerifyPr {
        /// Target branch the PR goes into (e.g. origin/stable/5.2.x); also accepted as --base
        #[arg(value_name = "BASE", required_unless_present = "base")]
        base_pos: Option<String>,
        #[arg(long, hide = true, conflicts_with = "base_pos")]
        base: Option<String>,
        /// Result to verify (default HEAD)
        #[arg(long)]
        head: Option<String>,
        /// The original change, if it is not marked in the commit messages
        #[arg(long)]
        source: Option<String>,
        /// apply | revert (default: inferred from the markers)
        #[arg(long)]
        kind: Option<String>,
        /// Fail when no original change is found
        #[arg(long)]
        require_source: bool,
        /// Also write the Markdown report to this file
        #[arg(long)]
        markdown: Option<PathBuf>,
        /// Skip the LLM verifier even if configured
        #[arg(long)]
        no_llm: bool,
        /// Run `command` requirements stored in notes (off by default: notes refs are not
        /// protected by branch protection, ADR 0014)
        #[arg(long)]
        trust_notes: bool,
    },
    /// Prove a PR's tests test its change: each new/changed test must fail on BASE without
    /// the change and pass with it (ADR 0017)
    ProvePr {
        /// Branch the PR goes into (e.g. main, origin/main); also accepted as --base
        #[arg(value_name = "BASE", required_unless_present = "base")]
        base_pos: Option<String>,
        #[arg(long, hide = true, conflicts_with = "base_pos")]
        base: Option<String>,
        /// The PR's result (default HEAD)
        #[arg(long)]
        head: Option<String>,
        /// Prove uncommitted changes against HEAD instead (your files and index are untouched)
        #[arg(long, conflicts_with = "head")]
        working_tree: bool,
        /// Every added/modified test must prove the change (default: at least one)
        #[arg(long)]
        require_all: bool,
        /// Also write the Markdown report to this file
        #[arg(long)]
        markdown: Option<PathBuf>,
    },
    /// Run all live requirements (the ledger) on the working tree or a revision
    Check {
        rev: Option<String>,
        /// Only requirements of this intention
        #[arg(long)]
        intent: Option<String>,
        /// Also evaluate judged requirements with the LLM
        #[arg(long)]
        judge: bool,
    },
    /// [experimental] Semantic diff: analyze uncommitted changes, or list intentions between revisions
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
    /// [experimental] Re-implement commits' intentions on the current branch (git first, then your agent; validated)
    CherryPick {
        #[arg(required = true)]
        commits: Vec<String>,
        #[command(flatten)]
        o: OpFlags,
    },
    /// [experimental] Merge a branch by composing intentions
    Merge {
        branch: String,
        /// Proceed even if intentions look incompatible
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        o: OpFlags,
    },
    /// [experimental] Replay this branch's intentions onto another base
    Rebase {
        onto: String,
        #[command(flatten)]
        o: OpFlags,
    },
    /// [experimental] Remove a commit's intention from the current branch
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
    /// [experimental] Create semantic records for commits made with plain git
    Annotate {
        rev: Option<String>,
        #[arg(short = 'n', long, default_value_t = 50)]
        limit: usize,
    },
    /// Serve the tool API over MCP (stdio)
    Mcp,
    /// [experimental] Serve the tool API over HTTP (loopback only). Clients must send
    /// `Authorization: Bearer $GITENT_TOKEN`; without GITENT_TOKEN a random token
    /// is generated and written to .git/gitent/serve-token (mode 0600).
    Serve {
        #[arg(long, default_value = "127.0.0.1:7341")]
        addr: String,
        /// Allow a non-loopback address (requires GITENT_TOKEN to be set explicitly)
        #[arg(long)]
        allow_remote: bool,
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
    use clap::{CommandFactory, FromArgMatches};
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    REQ_DRAFTS.with(|r| {
        if let Some(m) = matches.subcommand_matches("commit") {
            *r.borrow_mut() = requirement_drafts(m);
        }
    });
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
        ("test_selector", &cli.test_selector),
    ] {
        if let Some(v) = v {
            args[k] = json!(v);
        }
    }
    if let Some(m) = cli.max_attempts {
        args["max_attempts"] = json!(m);
    }
    if cli.trust_agent_commands {
        args["trust_agent_commands"] = json!(true);
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
        Cmd::Serve { addr, allow_remote } => {
            gitent::http::serve(&path, addr, *allow_remote)?;
            return Ok((json!({}), 0));
        }
        Cmd::Trust => {
            let v = trust(&path)?;
            if human {
                println!("{}", v["message"].as_str().unwrap_or(""));
            }
            return Ok((v, 0));
        }
        Cmd::Commit {
            message,
            intent_file,
            staged,
            require_llm,
            dry_run,
            retire,
            check_ledger,
            no_prove,
            allow_failing,
            ..
        } => {
            args["requirements"] = REQ_DRAFTS.with(|r| json!(*r.borrow()));
            args["retires"] = json!(retire
                .chunks(2)
                .map(|c| json!({"id": c[0], "reason": c.get(1).cloned().unwrap_or_default()}))
                .collect::<Vec<_>>());
            args["check_ledger"] = json!(check_ledger);
            if *no_prove {
                args["prove"] = json!("off");
            }
            args["allow_failing"] = json!(allow_failing);
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
        Cmd::Verify {
            source,
            result,
            onto,
            working_tree,
            kind,
            record,
            no_llm,
        } => {
            args["source"] = json!(source);
            if let Some(r) = result {
                args["result"] = json!(r);
            }
            if let Some(o) = onto {
                args["onto"] = json!(o);
            }
            args["working_tree"] = json!(working_tree);
            args["kind"] = json!(kind);
            args["record"] = json!(record);
            args["no_llm"] = json!(no_llm);
            ("verify", render_verify)
        }
        Cmd::ProvePr {
            base_pos,
            base,
            head,
            working_tree,
            require_all,
            markdown,
        } => {
            args["base"] = json!(base_pos.clone().or(base.clone()));
            if let Some(h) = head {
                args["head"] = json!(h);
            }
            args["working_tree"] = json!(working_tree);
            args["require_all"] = json!(require_all);
            MARKDOWN_OUT.with(|m| *m.borrow_mut() = markdown.clone());
            ("prove_pr", render_prove_pr)
        }
        Cmd::VerifyPr {
            base_pos,
            base,
            head,
            source,
            kind,
            require_source,
            markdown,
            no_llm,
            trust_notes,
        } => {
            args["base"] = json!(base_pos.clone().or(base.clone()));
            args["trust_notes"] = json!(trust_notes);
            for (k, v) in [("head", head), ("source", source), ("kind", kind)] {
                if let Some(v) = v {
                    args[k] = json!(v);
                }
            }
            args["require_source"] = json!(require_source);
            args["no_llm"] = json!(no_llm);
            MARKDOWN_OUT.with(|m| *m.borrow_mut() = markdown.clone());
            ("verify_pr", render_verify_pr)
        }
        Cmd::Check { rev, intent, judge } => {
            if let Some(r) = rev {
                args["rev"] = json!(r);
            }
            if let Some(i) = intent {
                args["intent"] = json!(i);
            }
            args["judge"] = json!(judge);
            ("check", render_check)
        }
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
    if let Some(p) = MARKDOWN_OUT.with(|m| m.borrow().clone()) {
        std::fs::write(&p, v["markdown"].as_str().unwrap_or(""))
            .with_context(|| format!("writing {}", p.display()))?;
    }
    if human {
        renderer(&v);
    }
    // 0 ok · 2 rejected/failed · 3 unverified (ADR 0013 §1) · 1 error
    let code =
        if v["verdict"] == "unverified" || v["result"] == "unverified" || v["result"] == "unproven"
        {
            3
        } else if gitent::mcp::is_failed(&v) {
            2
        } else {
            0
        };
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
        let (cmd, sel) = detect_test_command(&repo.workdir);
        cfg.validation.test_command = cmd;
        cfg.validation.test_selector = sel;
    }
    std::fs::create_dir_all(target.parent().unwrap())?;
    let header = format!(
        "# Gitent configuration (preset: {preset}). See docs/INTEGRATIONS.md.\n\
         # Local overrides: .git/gitent/config.toml. Env overrides: GITENT_EXECUTOR, GITENT_PROVIDER, ...\n\n"
    );
    let text = header + &cfg.to_toml();
    std::fs::write(&target, &text)?;
    if !local {
        // The user just chose this content (ADR 0016 §4).
        gitent::config::trust(&text, &repo.git_dir)?;
    }
    Ok(
        json!({"initialized": true, "config": target, "preset": preset, "message": format!(
        "Initialized gitent in {} (preset: {preset}, config: {}){}",
        repo.workdir.display(), target.display(),
        if cfg.validation.test_command.is_empty() { "\nhint: set validation.test_command so operations can be validated".to_string() } else { format!("\ntest command: {}", cfg.validation.test_command) })}),
    )
}

/// `gitent trust`: record the shared config's current content as trusted (ADR 0016 §4).
fn trust(path: &std::path::Path) -> Result<Value> {
    let repo = Repo::discover(path)?;
    let file = repo.workdir.join(".gitent/config.toml");
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("no shared config to trust ({})", file.display()))?;
    let t: toml::Table = toml::from_str(&text).context("parsing .gitent/config.toml")?;
    let hash = gitent::config::trust(&text, &repo.git_dir)?;
    let mut sets = serde_json::Map::new();
    for (s, k) in gitent::config::COMMAND_KEYS {
        if let Some(toml::Value::String(v)) = t.get(*s).and_then(|x| x.get(*k)) {
            if !v.trim().is_empty() {
                sets.insert(format!("{s}.{k}"), json!(v));
            }
        }
    }
    let mut msg = format!(
        "Trusted {} (sha256 {})",
        file.display(),
        &hash[..12.min(hash.len())]
    );
    for (k, v) in &sets {
        msg.push_str(&format!("\n  {k} = {}", v.as_str().unwrap_or("")));
    }
    Ok(json!({
        "trusted": true,
        "config": file,
        "sha256": hash,
        "sets": sets,
        "message": msg,
    }))
}

/// (test_command, test_selector) defaults for common project layouts. Selectors run one test
/// where the runner allows it (ADR 0013 §3); placeholders are shell-quoted by gitent.
fn detect_test_command(dir: &std::path::Path) -> (String, String) {
    let p = |a: &str, b: &str| (a.to_string(), b.to_string());
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    if dir.join("Cargo.toml").exists() {
        p("cargo test", "cargo test {test}")
    } else if dir.join("package.json").exists() {
        let pkg = read("package.json");
        if pkg.contains("\"vitest\"") {
            p("npx vitest run", "npx vitest run -t {test_esc}")
        } else if pkg.contains("\"mocha\"") {
            p("npm test --silent", "npx mocha -g {test_esc}")
        } else {
            p("npm test --silent", "npx jest -t {test_esc}")
        }
    } else if dir.join("go.mod").exists() {
        p("go test ./...", "go test ./... -run {test_re}")
    } else if dir.join("pyproject.toml").exists()
        || dir.join("setup.py").exists()
        || dir.join("setup.cfg").exists()
        || dir.join("tests").is_dir()
    {
        let uses_pytest = dir.join("conftest.py").exists()
            || dir.join("tests/conftest.py").exists()
            || dir.join("pytest.ini").exists()
            || read("pyproject.toml").contains("[tool.pytest")
            || read("pyproject.toml").contains("pytest")
            || read("setup.cfg").contains("[tool:pytest]")
            || read("tox.ini").contains("[pytest]")
            || [
                "requirements.txt",
                "requirements-dev.txt",
                "requirements_test.txt",
            ]
            .iter()
            .any(|f| read(f).contains("pytest"));
        if uses_pytest {
            p(
                "python3 -m pytest -q",
                "python3 -m pytest -q {file} -k {test}",
            )
        } else {
            p(
                "python3 -m unittest discover -q",
                "python3 -m unittest -q -k {test} {module}",
            )
        }
    } else {
        p("", "")
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
    for q in r["requirements"].as_array().into_iter().flatten() {
        let how = match s(q, "kind") {
            "test" => format!("test {}", s(q, "test")),
            "command" => format!("`{}`", s(&q["check"], "run")),
            _ => "judged".to_string(),
        };
        let proof = match q.get("proof") {
            Some(p) if p.is_object() => {
                format!("  [before: {}, after: {}]", s(p, "before"), s(p, "after"))
            }
            _ => String::new(),
        };
        let label = if s(q, "category") == "constraint" {
            "keep"
        } else {
            "require"
        };
        println!(
            "{label:<8} {} {} ({how}){proof}",
            s(q, "id"),
            s(q, "statement")
        );
    }
    for q in r["retires"].as_array().into_iter().flatten() {
        println!("retires  {} — {}", s(q, "id"), s(q, "reason"));
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

thread_local! {
    static MARKDOWN_OUT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
    static REQ_DRAFTS: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Pair each --test/--check with the closest preceding --require (by argument position).
fn requirement_drafts(m: &clap::ArgMatches) -> Vec<Value> {
    let idx = |id: &str| -> Vec<(usize, String)> {
        match (m.indices_of(id), m.get_many::<String>(id)) {
            (Some(i), Some(v)) => i.zip(v.cloned()).collect(),
            _ => vec![],
        }
    };
    let reqs = idx("require");
    let mut drafts: Vec<Value> = reqs
        .iter()
        .map(|(_, st)| json!({"statement": st}))
        .collect();
    for (key, items) in [("test", idx("test")), ("check", idx("check"))] {
        for (pos, val) in items {
            let owner = reqs.iter().rposition(|(i, _)| *i < pos);
            let payload = if key == "test" {
                json!(val)
            } else {
                json!({"run": val, "expect_exit": 0, "timeout_secs": 120})
            };
            match owner {
                Some(o) if drafts[o].get(key).is_none() => drafts[o][key] = payload,
                _ => {
                    let mut d = json!({"statement": ""});
                    d[key] = payload;
                    drafts.push(d);
                }
            }
        }
    }
    drafts
}

/// ANSI colour for human output on a terminal (off for pipes, files and NO_COLOR).
fn paint(text: &str, code: &str) -> String {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}
fn ok(t: &str) -> String {
    paint(t, "1;32")
}
fn bad(t: &str) -> String {
    paint(t, "1;31")
}

fn render_verify(v: &Value) {
    let result = s(v, "result");
    println!(
        "{} {} of {} onto {} → {}",
        match s(v, "verdict") {
            "verified" => ok("✓ verified:"),
            "unverified" => paint("? unverified (needs review):", "1;33"),
            _ => bad("✗ rejected:"),
        },
        s(v, "kind"),
        short(s(v, "source")),
        short(s(v, "base")),
        if result == "working-tree" {
            "working tree".to_string()
        } else {
            short(result)
        }
    );
    for c in v["validation"]["checks"].as_array().into_iter().flatten() {
        let mark = if c["passed"] == true {
            ok("✓")
        } else if c["review"] == true {
            paint("?", "1;33")
        } else {
            bad("✗")
        };
        let d = s(c, "detail");
        let first = d
            .lines()
            .take(if c["passed"] == true { 1 } else { 8 })
            .collect::<Vec<_>>()
            .join("\n      ");
        println!("  {mark} {:<26} {first}", s(c, "name"));
    }
    for w in v["validation"]["warnings"].as_array().into_iter().flatten() {
        println!("  warning: {}", w.as_str().unwrap_or(""));
    }
    if v["recorded"] == true {
        println!(
            "recorded semantic record {} on {}",
            s(&v["record"], "intent_id"),
            short(result)
        );
    }
}

fn render_prove_pr(v: &Value) {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        println!("{}", s(v, "markdown"));
        return;
    }
    for t in v["tests"].as_array().into_iter().flatten() {
        let (mark, what) = match s(t, "status") {
            "proven" => (ok("✓"), "proves the change"),
            "proven_api" => (
                ok("✓"),
                "fails before only because the code it calls did not exist",
            ),
            "passes_without_change" => (
                paint("?", "1;33"),
                "passes WITHOUT the change: does not test it",
            ),
            "fails_on_pr" => (bad("✗"), "fails on the PR"),
            _ => ("–".to_string(), "not run"),
        };
        println!(
            "  {mark} {:<40} without: {:<5} with: {:<5} {what}",
            format!("{}::{}", s(t, "file"), s(t, "test")),
            s(t, "without_change"),
            s(t, "with_change")
        );
    }
    for (k, label) in [
        ("skipped_existing", "skipped existing test"),
        ("deleted_tests", "deleted existing test"),
        ("notes", "note"),
    ] {
        for x in v[k].as_array().into_iter().flatten() {
            println!("  {label}: {}", x.as_str().unwrap_or(""));
        }
    }
    let head = match s(v, "result") {
        "proven" => ok("✓ proven:"),
        "neutral" => "– neutral:".to_string(),
        "unproven" => paint("? unproven:", "1;33"),
        _ => bad("✗ rejected:"),
    };
    println!("{head} {}", s(v, "reason"));
}

fn render_verify_pr(v: &Value) {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        // Piped (e.g. into a PR comment): the Markdown report.
        println!("{}", s(v, "markdown"));
        return;
    }
    let verifications = v["verifications"].as_array().cloned().unwrap_or_default();
    if verifications.is_empty() {
        let n = s(v, "note");
        match s(v, "result") {
            "rejected" => println!("{} {n}", bad("✗ rejected:")),
            _ => println!("– nothing to verify: {n}"),
        }
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            println!("  {e}");
        }
        return;
    }
    for x in &verifications {
        println!("{} ({})", s(x, "source_subject"), s(x, "marker"));
        render_verify(x);
        println!();
    }
    match s(v, "result") {
        "verified" => println!("{}", ok("PR verified")),
        "unverified" => println!("{}", paint("PR not proven: needs review", "1;33")),
        _ => println!("{}", bad("PR rejected")),
    }
}

fn render_check(v: &Value) {
    for r in v["results"].as_array().into_iter().flatten() {
        let mark = match s(r, "status") {
            "pass" => ok("✓"),
            "fail" => bad("✗"),
            "uncertain" => paint("?", "1;33"),
            _ => "-".to_string(),
        };
        println!(
            "{mark} {} \"{}\" — from \"{}\" ({})",
            s(r, "id"),
            s(r, "statement"),
            s(r, "intent_title"),
            short(s(r, "introduced_by"))
        );
        if s(r, "status") == "fail" {
            for l in s(r, "detail").lines().take(6) {
                println!("      {l}");
            }
        }
    }
    let c = &v["counts"];
    println!(
        "{} passed, {} failed, {} skipped, {} uncertain",
        c["pass"], c["fail"], c["skipped"], c["uncertain"]
    );
}

fn render_commit(v: &Value) {
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        println!("✗ {e}");
        return;
    }
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
            "{} {} updated to {}",
            ok("✓"),
            s(v, "branch_ref").trim_start_matches("refs/heads/"),
            short(s(v, "result_commit"))
        ),
        "awaiting_host" => {
            println!("brief:    {}", s(v, "brief"));
            println!("worktree: {}", s(v, "worktree"));
            println!("{}", s(v, "next"));
        }
        "failed" => println!("{} {}", bad("✗"), s(v, "error")),
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
