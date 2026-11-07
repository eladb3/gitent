//! The tool surface shared by the CLI (`--json`), the MCP server and the HTTP API.
//! Every tool takes a JSON object and returns a JSON value.

use crate::model::IntentDraft;
use crate::ops::commit::{self, CommitOptions};
use crate::ops::plan::{self, OpOptions};
use crate::ops::{engine, Overrides, Session};
use crate::repo::short;
use crate::store;
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::path::Path;

/// Who is calling. Agents default to the `host` executor (they do the semantic work).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Cli,
    Agent,
}

fn arg_str(a: &Value, k: &str) -> Option<String> {
    a.get(k)
        .and_then(|v| v.as_str())
        .map(String::from)
        .filter(|s| !s.is_empty())
}

fn arg_bool(a: &Value, k: &str) -> bool {
    a.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Version of the tool input schemas (`tools/list`, `GET /v1/tools`, `gitent schema`).
/// @2: command-selecting arguments removed from agent surfaces (ADR 0014 §2).
pub const TOOLS_SCHEMA: &str = "gitent/tools@2";

/// Arguments that select a command (or the program that runs one). Only the CLI
/// may set them; MCP/HTTP callers get them from config (ADR 0014 §2).
pub const CLI_ONLY_ARGS: &[&str] = &[
    "executor_command",
    "test_command",
    "test_selector",
    "provider",
];

fn overrides(a: &Value, surface: Surface) -> Overrides {
    let cli = |k: &str| {
        if surface == Surface::Cli {
            arg_str(a, k)
        } else {
            None
        }
    };
    let mut ov = Overrides {
        executor: arg_str(a, "executor"),
        executor_command: cli("executor_command"),
        provider: cli("provider"),
        test_command: cli("test_command"),
        test_selector: cli("test_selector"),
        max_attempts: a
            .get("max_attempts")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    };
    if surface == Surface::Agent
        && ov.executor.is_none()
        && std::env::var("GITENT_EXECUTOR").is_err()
    {
        ov.executor = Some("host".into());
    }
    ov
}

fn op_options(a: &Value) -> OpOptions {
    OpOptions {
        strict: arg_bool(a, "strict"),
        force: arg_bool(a, "force"),
        no_git: arg_bool(a, "no_git"),
    }
}

pub fn call(path: &Path, tool: &str, args: &Value, surface: Surface) -> Result<Value> {
    let tool = tool.strip_prefix("gitent_").unwrap_or(tool);
    if tool == "schema" {
        let mut v = crate::model::json_schema();
        v["x-gitent-tools"] = json!({"schema": TOOLS_SCHEMA, "tools": tool_defs()});
        return Ok(v);
    }
    if surface == Surface::Agent {
        if let Some(k) = CLI_ONLY_ARGS.iter().find(|k| args.get(**k).is_some()) {
            bail!(
                "`{k}` cannot be set through the tool API (ADR 0014): configure it in .gitent/config.toml or pass it on the CLI"
            );
        }
    }
    let s = Session::open(path, &overrides(args, surface))?;
    match tool {
        "status" => status(&s),
        "log" => log(
            &s,
            arg_str(args, "rev").as_deref(),
            args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize,
        ),
        "show" => show(&s, &arg_str(args, "rev").unwrap_or_else(|| "HEAD".into())),
        "diff" => diff(
            &s,
            arg_str(args, "from").as_deref(),
            arg_str(args, "to").as_deref(),
        ),
        "commit" => {
            let intent: Option<IntentDraft> = match args.get("intent") {
                Some(Value::Null) | None => None,
                Some(v) => Some(
                    serde_json::from_value(v.clone())
                        .map_err(|e| anyhow!("invalid `intent`: {e}"))?,
                ),
            };
            let r = commit::commit(
                &s,
                &CommitOptions {
                    message: arg_str(args, "message"),
                    intent,
                    staged_only: arg_bool(args, "staged_only"),
                    require_llm: arg_bool(args, "require_llm"),
                    dry_run: arg_bool(args, "dry_run"),
                    requirements: match args.get("requirements") {
                        Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                            .map_err(|e| anyhow!("invalid `requirements`: {e}"))?,
                        _ => vec![],
                    },
                    retires: match args.get("retires") {
                        Some(v) if !v.is_null() => serde_json::from_value(v.clone())
                            .map_err(|e| anyhow!("invalid `retires`: {e}"))?,
                        _ => vec![],
                    },
                    check_ledger: arg_bool(args, "check_ledger"),
                    prove: arg_str(args, "prove"),
                    allow_failing: args
                        .get("allow_failing")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default(),
                    agent_supplied: surface == Surface::Agent,
                },
            )?;
            let mut v = json!({"commit": r.commit, "record": r.record});
            if !r.ledger_results.is_empty() {
                v["ledger"] = json!(r.ledger_results);
            }
            if let Some(why) = r.rejected {
                v["state"] = json!("failed");
                v["error"] = json!(format!("commit rejected (nothing was changed):\n{why}"));
            }
            Ok(v)
        }
        "verify" => {
            let src = arg_str(args, "source").ok_or_else(|| anyhow!("`source` is required"))?;
            let mut s2 = s;
            if arg_bool(args, "no_llm") {
                s2.provider = None;
            }
            crate::ops::verify::verify(
                &s2,
                &crate::ops::verify::VerifyOptions {
                    source: src,
                    result: arg_str(args, "result"),
                    onto: arg_str(args, "onto"),
                    working_tree: arg_bool(args, "working_tree"),
                    kind: arg_str(args, "kind").unwrap_or_default(),
                    record: arg_bool(args, "record"),
                },
            )
        }
        "verify_pr" => {
            let base = arg_str(args, "base").ok_or_else(|| anyhow!("`base` is required"))?;
            let mut s2 = s;
            if arg_bool(args, "no_llm") {
                s2.provider = None;
            }
            crate::ops::verify_pr::verify_pr(
                &s2,
                &crate::ops::verify_pr::PrOptions {
                    base,
                    head: arg_str(args, "head"),
                    source: arg_str(args, "source"),
                    kind: arg_str(args, "kind"),
                    require_source: arg_bool(args, "require_source"),
                },
            )
        }
        "check" => check(
            &s,
            arg_str(args, "rev").as_deref(),
            arg_str(args, "intent").as_deref(),
            arg_bool(args, "judge"),
        ),
        "cherry_pick" => {
            let mut revs: Vec<String> = args
                .get("commits")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if let Some(c) = arg_str(args, "commit") {
                revs.push(c);
            }
            if revs.is_empty() {
                bail!("`commit` is required");
            }
            Ok(plan::cherry_pick(&s, &revs, &op_options(args))?.summary())
        }
        "revert" => {
            let rev = arg_str(args, "commit").ok_or_else(|| anyhow!("`commit` is required"))?;
            Ok(plan::revert(&s, &rev, &op_options(args))?.summary())
        }
        "merge" => {
            let b = arg_str(args, "branch").ok_or_else(|| anyhow!("`branch` is required"))?;
            plan::merge(&s, &b, &op_options(args))
        }
        "rebase" => {
            let b = arg_str(args, "onto").ok_or_else(|| anyhow!("`onto` is required"))?;
            plan::rebase(&s, &b, &op_options(args))
        }
        "continue" => Ok(engine::resume(&s, arg_str(args, "op_id").as_deref())?.summary()),
        "abort" => Ok(engine::abort(&s, arg_str(args, "op_id").as_deref())?.summary()),
        "ops" => {
            Ok(json!({"operations": s.list_ops()?.iter().map(|o| o.summary()).collect::<Vec<_>>()}))
        }
        "annotate" => annotate(
            &s,
            arg_str(args, "rev").as_deref(),
            args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize,
        ),
        other => bail!("unknown tool `{other}`"),
    }
}

fn status(s: &Session) -> Result<Value> {
    let head = s.repo.head_commit()?;
    let head_id = head.as_ref().map(|c| c.id());
    let rec = head_id.and_then(|o| store::get(&s.repo, o));
    let open: Vec<Value> = s
        .list_ops()?
        .into_iter()
        .filter(|o| o.state.is_open())
        .map(|o| o.summary())
        .collect();
    Ok(json!({
        "repository": s.repo.workdir,
        "branch": s.repo.head_ref_name().ok().map(|r| r.trim_start_matches("refs/heads/").to_string()),
        "head": head_id.map(|o| o.to_string()),
        "head_intent": rec.map(|r| json!({"intent_id": r.intent_id, "title": r.title})),
        "dirty": s.repo.dirty_paths()?,
        "open_operations": open,
        "config": {
            "llm_provider": s.cfg.llm.provider,
            "executor": s.cfg.executor.kind,
            "executor_command": s.cfg.executor.command,
            "test_command": s.cfg.validation.test_command,
            "max_attempts": s.cfg.validation.max_attempts,
        }
    }))
}

pub fn log_entry(s: &Session, oid: git2::Oid) -> Result<Value> {
    let c = s.repo.git.find_commit(oid)?;
    let summary = c.summary().unwrap_or("").to_string();
    let mut v = json!({"commit": oid.to_string(), "short": short(&oid.to_string()), "summary": summary,
                       "parents": c.parent_ids().map(|p| p.to_string()).collect::<Vec<_>>()});
    if let Some(r) = store::get(&s.repo, oid) {
        v["intent_id"] = json!(r.intent_id);
        v["title"] = json!(r.title);
        v["kind"] = json!(r.kind);
        v["operation"] = json!(r.provenance.operation);
        v["executor"] = json!(r.provenance.executor);
        v["confidence"] = json!(r.confidence);
        v["analysis"] = json!(r.provenance.analysis.method);
        if let Some(d) = &r.provenance.derived_from {
            v["derived_from"] = json!(d);
        }
        if let Some(d) = &r.provenance.reverts {
            v["reverts"] = json!(d);
        }
        if !r.provenance.composes.is_empty() {
            v["composes"] = json!(r.provenance.composes);
        }
        if let Some(val) = &r.provenance.validation {
            v["validated"] = json!(val.passed);
        }
        v["requirements"] = json!(r.requirements.len());
        if !r.retires.is_empty() {
            v["retires"] = json!(r.retires);
        }
    }
    Ok(v)
}

fn log(s: &Session, rev: Option<&str>, limit: usize) -> Result<Value> {
    let start = match rev {
        Some(r) => s.repo.resolve(r)?,
        None => match s.repo.head_commit()? {
            Some(c) => c.id(),
            None => return Ok(json!({"entries": []})),
        },
    };
    let mut walk = s.repo.git.revwalk()?;
    walk.push(start)?;
    walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
    let mut entries = vec![];
    for oid in walk.take(limit) {
        entries.push(log_entry(s, oid?)?);
    }
    Ok(json!({"entries": entries}))
}

fn show(s: &Session, rev: &str) -> Result<Value> {
    let oid = store::resolve(&s.repo, rev)?;
    let c = s.repo.git.find_commit(oid)?;
    let d = s.repo.diff_commit(oid)?;
    let rec = store::get(&s.repo, oid);
    let realizations: Vec<String> = match &rec {
        Some(r) => store::find_by_intent(&s.repo, &r.intent_id)?
            .into_iter()
            .map(|(o, _)| o.to_string())
            .collect(),
        None => vec![],
    };
    Ok(json!({
        "commit": oid.to_string(),
        "message": c.message().unwrap_or(""),
        "files": d.files,
        "record": rec,
        "realizations": realizations,
    }))
}

fn diff(s: &Session, from: Option<&str>, to: Option<&str>) -> Result<Value> {
    match (from, to) {
        (None, None) => {
            let (d, rec) = commit::semantic_diff(s)?;
            Ok(json!({"files": d.files, "record": rec, "patch": d.patch}))
        }
        (from, to) => {
            let to_oid = s.repo.resolve(to.unwrap_or("HEAD"))?;
            let from_oid = s.repo.resolve(from.unwrap_or("HEAD"))?;
            let base = s.repo.merge_base(from_oid, to_oid)?;
            let mut intents = vec![];
            for c in s.repo.commits_between(base, to_oid)? {
                intents.push(log_entry(s, c)?);
            }
            Ok(
                json!({"from": from_oid.to_string(), "to": to_oid.to_string(), "base": base.to_string(), "intents": intents}),
            )
        }
    }
}

fn annotate(s: &Session, rev: Option<&str>, limit: usize) -> Result<Value> {
    let start = s.repo.resolve(rev.unwrap_or("HEAD"))?;
    let mut walk = s.repo.git.revwalk()?;
    walk.push(start)?;
    let mut done = vec![];
    for oid in walk.take(limit) {
        let oid = oid?;
        if store::get(&s.repo, oid).is_none() && s.repo.git.find_commit(oid)?.parent_count() <= 1 {
            let r = store::record_for(&s.repo, s.provider(), oid)?;
            done.push(json!({"commit": oid.to_string(), "intent_id": r.intent_id, "title": r.title, "analysis": r.provenance.analysis.method}));
        }
    }
    Ok(json!({"annotated": done}))
}

fn check(s: &Session, rev: Option<&str>, intent: Option<&str>, judge: bool) -> Result<Value> {
    use crate::requirements::{self, Runner, TempTree};
    let tip = match rev {
        Some(r) => s.repo.resolve(r)?,
        None => match s.repo.head_commit()? {
            Some(c) => c.id(),
            None => return Ok(json!({"passed": true, "results": []})),
        },
    };
    let mut live = requirements::ledger(&s.repo, tip)?;
    if let Some(i) = intent {
        live.retain(|l| l.intent_id == i);
    }
    let _tmp;
    let dir = match rev {
        Some(_) => {
            _tmp = TempTree::new(&s.repo, tip, "check")?;
            _tmp.path.clone()
        }
        None => s.repo.workdir.clone(),
    };
    let mut results = Runner::new(&dir, &s.cfg).run_all(&live);
    if judge {
        if let Some(p) = s.provider() {
            for (r, l) in results.iter_mut().zip(live.iter()) {
                if l.req.kind == crate::model::ReqKind::Judged {
                    let files = store::get(&s.repo, git2::Oid::from_str(&l.commit)?)
                        .map(|rec| rec.affected.files)
                        .unwrap_or_default();
                    let (st, detail) = requirements::judge(
                        p,
                        &l.req.statement,
                        &dir,
                        &files,
                        s.cfg.validation.verifier_threshold,
                    );
                    r.status = st;
                    r.detail = detail;
                }
            }
        }
    }
    let failed = results.iter().filter(|r| r.status == "fail").count();
    Ok(json!({
        "rev": tip.to_string(),
        "working_tree": rev.is_none(),
        "passed": failed == 0,
        "state": if failed == 0 { "succeeded" } else { "failed" },
        "counts": {
            "pass": results.iter().filter(|r| r.status == "pass").count(),
            "fail": failed,
            "skipped": results.iter().filter(|r| r.status == "skipped").count(),
            "uncertain": results.iter().filter(|r| r.status == "uncertain").count(),
        },
        "results": results,
    }))
}

/// Tool definitions (MCP `tools/list`, HTTP `GET /v1/tools`).
pub fn tool_defs() -> Vec<Value> {
    let exec_props = json!({
        "executor": {"type": "string", "enum": ["host", "command", "builtin"], "description": "Who changes code when needed. Default for agents: host (you do it, then call gitent_continue)."},
        "strict": {"type": "boolean", "description": "Refuse to finalize if there are warnings"},
        "no_git": {"type": "boolean", "description": "Skip the deterministic git attempt"},
        "max_attempts": {"type": "integer"}
    });
    let with = |extra: Value, required: &[&str]| -> Value {
        let mut props = exec_props.as_object().unwrap().clone();
        for (k, v) in extra.as_object().unwrap() {
            props.insert(k.clone(), v.clone());
        }
        json!({"type": "object", "properties": props, "required": required})
    };
    let obj = |props: Value, required: &[&str]| json!({"type": "object", "properties": props, "required": required});
    vec![
        json!({"name": "gitent_status", "description": "Branch, HEAD intention, dirty files, open operations and configuration.", "inputSchema": obj(json!({}), &[])}),
        json!({"name": "gitent_log", "description": "History as semantic intentions with provenance.", "inputSchema": obj(json!({"rev": {"type": "string"}, "limit": {"type": "integer"}}), &[])}),
        json!({"name": "gitent_show", "description": "Full semantic record of a commit or intent id (int_…), plus all its realisations.", "inputSchema": obj(json!({"rev": {"type": "string"}}), &[])}),
        json!({"name": "gitent_diff", "description": "Without args: analyze uncommitted changes into an intention (no commit). With from/to: list intentions between revisions.", "inputSchema": obj(json!({"from": {"type": "string"}, "to": {"type": "string"}}), &[])}),
        json!({"name": "gitent_commit", "description": "Commit all changes (or staged only) with a semantic record. Pass `intent` (see gitent_schema $defs.intent_draft) when you know why you made the change; Gitent checks it against the diff.", "inputSchema": obj(json!({
            "message": {"type": "string"}, "intent": {"type": "object"}, "staged_only": {"type": "boolean"},
            "require_llm": {"type": "boolean"}, "dry_run": {"type": "boolean"},
            "requirements": {"type": "array", "description": "Requirements this change must satisfy: [{statement, category?: behavior|constraint, test?: test name, check?: {run, expect_exit}}]. Every test you add becomes one automatically.", "items": {"type": "object"}},
            "retires": {"type": "array", "description": "Live requirements this change deliberately ends: [{id, reason}]", "items": {"type": "object"}},
            "check_ledger": {"type": "boolean", "description": "Reject the commit if it breaks any live requirement"},
            "prove": {"type": "string", "enum": ["warn", "require", "off"]}}), &[])}),
        json!({"name": "gitent_verify", "description": "Check that an existing result (a commit, or your uncommitted working tree) correctly moves a change: the change's requirements, the target's requirements and tests, nothing resurrected/deleted/weakened. Use before committing or merging agent-made backports, merges and reverts. No agent is invoked.", "inputSchema": obj(json!({
            "source": {"type": "string", "description": "The change that was moved (commit or int_…); for merge, the merged branch"},
            "result": {"type": "string", "description": "Commit to verify (default HEAD)"},
            "onto": {"type": "string", "description": "Target before the change (default: result's first parent)"},
            "working_tree": {"type": "boolean", "description": "Verify uncommitted changes against HEAD"},
            "kind": {"type": "string", "enum": ["apply", "revert", "merge"]},
            "record": {"type": "boolean", "description": "Attach a semantic record to the result if verified"},
            "no_llm": {"type": "boolean"}}), &["source"])}),
        json!({"name": "gitent_verify_pr", "description": "Find the backports/cherry-picks/reverts among the commits base..head (via commit markers) and verify each. Returns result verified|rejected|neutral and a Markdown report.", "inputSchema": obj(json!({
            "base": {"type": "string", "description": "Target branch (e.g. origin/stable/5.2.x)"},
            "head": {"type": "string"}, "source": {"type": "string"}, "kind": {"type": "string", "enum": ["apply", "revert"]},
            "require_source": {"type": "boolean"}, "no_llm": {"type": "boolean"}}), &["base"])}),
        json!({"name": "gitent_check", "description": "Run all live requirements (the ledger) on the working tree or a revision; names the intention behind any failure.", "inputSchema": obj(json!({"rev": {"type": "string"}, "intent": {"type": "string"}, "judge": {"type": "boolean"}}), &[])}),
        json!({"name": "gitent_cherry_pick", "description": "Re-implement a commit's intention on the current branch. Tries git first, validates, falls back to an agent. With executor=host returns state=awaiting_host with a worktree and brief: edit there, then call gitent_continue.", "inputSchema": with(json!({"commit": {"type": "string"}, "commits": {"type": "array", "items": {"type": "string"}}}), &[])}),
        json!({"name": "gitent_merge", "description": "Merge a branch by composing intentions from both sides; checks compatibility, validates the result.", "inputSchema": with(json!({"branch": {"type": "string"}, "force": {"type": "boolean", "description": "Proceed even if intentions look incompatible"}}), &["branch"])}),
        json!({"name": "gitent_rebase", "description": "Replay the current branch's intentions onto another base.", "inputSchema": with(json!({"onto": {"type": "string"}}), &["onto"])}),
        json!({"name": "gitent_revert", "description": "Remove a commit's intention from the current branch, even after later refactors.", "inputSchema": with(json!({"commit": {"type": "string"}}), &["commit"])}),
        json!({"name": "gitent_continue", "description": "Validate the host agent's edits in the operation's worktree and continue (commit or return feedback).", "inputSchema": obj(json!({"op_id": {"type": "string"}}), &[])}),
        json!({"name": "gitent_abort", "description": "Abort an operation and remove its worktree. The branch is never changed by an unfinished operation.", "inputSchema": obj(json!({"op_id": {"type": "string"}}), &[])}),
        json!({"name": "gitent_ops", "description": "List operations (journal).", "inputSchema": obj(json!({}), &[])}),
        json!({"name": "gitent_annotate", "description": "Create semantic records for commits made with plain git.", "inputSchema": obj(json!({"rev": {"type": "string"}, "limit": {"type": "integer"}}), &[])}),
        json!({"name": "gitent_schema", "description": "JSON schema of semantic records and of the intent draft accepted by gitent_commit.", "inputSchema": obj(json!({}), &[])}),
    ]
}
