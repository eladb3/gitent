//! Semantic analysis: turn an actual diff into a structured intention.
//!
//! Three sources, in order of preference: host-supplied draft, LLM, heuristic.
//! Whatever the source, deterministic facts (files, symbols, tests) come from
//! the diff, and claims are checked against it.

use crate::diff;
use crate::llm::{extract_json, Message, Provider};
use crate::model::*;
use crate::repo::DiffInfo;
use anyhow::{bail, Result};

pub const MAX_DIFF_CHARS: usize = 60_000;

const SYSTEM: &str = r#"You are the semantic analyzer of Gitent, an intention-tracking version control system.
Given a code change (unified diff) and the author's message, describe the change as a structured
INTENTION that another engineer or agent could re-implement on a different version of the code base,
even if functions were renamed, moved, or restructured there.

Respond with ONE JSON object and nothing else:
{
  "title": "imperative summary, <= 72 chars",
  "kind": "feature|fix|refactor|perf|test|docs|chore",
  "intent": "WHY the change exists (the goal), 1-3 sentences",
  "behavior": "the observable behaviour after the change, precise enough to test (inputs, outputs, edge cases)",
  "changes": [{"summary": "atomic sub-change", "files": ["path"], "symbols": ["name"]}],
  "symbols": [{"name": "identifier", "kind": "function|class|type|variable|module", "file": "path"}],
  "constraints": ["what must be preserved, e.g. public signatures, error types, performance bounds"],
  "acceptance": ["behavioural statements that must be true once the intention is realised"],
  "dependencies": ["preconditions in the code base this change relies on"],
  "confidence": 0.0-1.0
}
Only mention files and symbols that appear in the diff. Describe behaviour, not line edits."#;

pub fn analyzer_prompt(message: &str, d: &DiffInfo) -> String {
    let files: Vec<String> = d
        .files
        .iter()
        .map(|f| format!("- {} ({})", f.path, f.status))
        .collect();
    let mut patch = d.patch.clone();
    if patch.len() > MAX_DIFF_CHARS {
        let mut cut = MAX_DIFF_CHARS;
        while !patch.is_char_boundary(cut) {
            cut -= 1;
        }
        patch.truncate(cut);
        patch.push_str("\n…(diff truncated)…\n");
    }
    format!(
        "Author message:\n{message}\n\nFiles:\n{}\n\nDiff:\n```diff\n{patch}```",
        files.join("\n")
    )
}

pub struct Analysis {
    pub draft: IntentDraft,
    pub info: AnalysisInfo,
}

/// Produce a draft from a host-supplied intent, the LLM, or heuristics.
pub fn analyze(
    provider: Option<&dyn Provider>,
    host: Option<IntentDraft>,
    message: &str,
    d: &DiffInfo,
    require_llm: bool,
) -> Result<Analysis> {
    if let Some(h) = host {
        if h.title.trim().is_empty() || h.intent.trim().is_empty() {
            bail!("supplied intent must have non-empty `title` and `intent`");
        }
        return Ok(Analysis {
            draft: h,
            info: AnalysisInfo {
                method: "host".into(),
                ..Default::default()
            },
        });
    }
    if let Some(p) = provider {
        let res = p
            .complete(SYSTEM, &[Message::user(analyzer_prompt(message, d))])
            .and_then(|t| extract_json(&t))
            .and_then(|v| Ok(serde_json::from_value::<IntentDraft>(normalize_draft(v))?));
        match res {
            Ok(draft) if !draft.title.trim().is_empty() => {
                return Ok(Analysis {
                    draft,
                    info: AnalysisInfo {
                        method: "llm".into(),
                        provider: p.name(),
                        warnings: vec![],
                    },
                })
            }
            Ok(_) if require_llm => bail!("LLM analysis returned an empty title"),
            Err(e) if require_llm => bail!("LLM analysis failed: {e:#}"),
            Ok(_) => {
                return Ok(heuristic(
                    message,
                    d,
                    vec!["llm returned empty title; used heuristic".into()],
                ))
            }
            Err(e) => {
                return Ok(heuristic(
                    message,
                    d,
                    vec![format!("llm analysis failed, used heuristic: {e:#}")],
                ))
            }
        }
    }
    if require_llm {
        bail!("--require-llm given but no LLM provider is configured (llm.provider = none)");
    }
    Ok(heuristic(message, d, vec![]))
}

/// Coerce common LLM deviations (strings instead of lists, symbol strings) into the draft shape.
fn normalize_draft(mut v: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    if let Some(o) = v.as_object_mut() {
        for k in ["constraints", "acceptance", "dependencies"] {
            if let Some(Value::String(s)) = o.get(k).cloned() {
                o.insert(k.into(), Value::Array(vec![Value::String(s)]));
            }
        }
        if let Some(Value::Array(syms)) = o.get("symbols").cloned() {
            let fixed: Vec<Value> = syms
                .into_iter()
                .map(|s| match s {
                    Value::String(n) => serde_json::json!({"name": n}),
                    other => other,
                })
                .collect();
            o.insert("symbols".into(), Value::Array(fixed));
        }
        if let Some(c) = o
            .get("confidence")
            .and_then(|c| c.as_str())
            .and_then(|s| s.parse::<f64>().ok())
        {
            o.insert("confidence".into(), serde_json::json!(c));
        }
    }
    v
}

pub fn heuristic(message: &str, d: &DiffInfo, warnings: Vec<String>) -> Analysis {
    let mut lines = message.lines();
    let title = lines.next().unwrap_or("").trim().to_string();
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let (kind, title_clean) = conventional_prefix(&title);
    let title = if title_clean.is_empty() {
        summarize_files(d)
    } else {
        title_clean
    };
    let draft = IntentDraft {
        intent: if body.is_empty() { title.clone() } else { body },
        title,
        kind: kind.to_string(),
        behavior: String::new(),
        changes: d
            .files
            .iter()
            .map(|f| SubChange {
                summary: format!("{} {}", f.status, f.path),
                files: vec![f.path.clone()],
                symbols: vec![],
            })
            .collect(),
        confidence: Some(0.3),
        ..Default::default()
    };
    Analysis {
        draft,
        info: AnalysisInfo {
            method: "heuristic".into(),
            provider: String::new(),
            warnings,
        },
    }
}

fn conventional_prefix(title: &str) -> (&'static str, String) {
    if let Some((pre, rest)) = title.split_once(':') {
        let p = pre
            .split('(')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let kind = match p.as_str() {
            "feat" | "feature" => Some("feature"),
            "fix" => Some("fix"),
            "refactor" => Some("refactor"),
            "perf" => Some("perf"),
            "test" | "tests" => Some("test"),
            "docs" => Some("docs"),
            "chore" | "build" | "ci" => Some("chore"),
            _ => None,
        };
        if let Some(k) = kind {
            return (k, rest.trim().to_string());
        }
    }
    ("feature", title.to_string())
}

fn summarize_files(d: &DiffInfo) -> String {
    match d.files.len() {
        0 => "Empty change".into(),
        1 => format!("Update {}", d.files[0].path),
        n => format!("Update {} and {} other files", d.files[0].path, n - 1),
    }
}

/// Combine a draft with deterministic facts from the diff into a record.
/// `realization` and most of `provenance` are filled in by the caller.
pub fn build_record(a: Analysis, d: &DiffInfo, operation: &str) -> SemanticChange {
    let Analysis { draft, mut info } = a;
    let files: Vec<String> = d.files.iter().map(|f| f.path.clone()).collect();
    let mut symbols = diff::symbols_from_patch(&d.patch);
    for s in &draft.symbols {
        if s.name.trim().is_empty() || symbols.iter().any(|x| x.name == s.name) {
            continue;
        }
        let verified = d.patch.contains(&s.name);
        if !verified {
            info.warnings.push(format!(
                "claimed symbol `{}` does not occur in the diff",
                s.name
            ));
        }
        let file = if files.contains(&s.file) {
            s.file.clone()
        } else {
            String::new()
        };
        symbols.push(Symbol {
            name: s.name.clone(),
            kind: s.kind.clone(),
            file,
            verified,
        });
    }
    let mut changes = vec![];
    for c in draft.changes {
        let (known, unknown): (Vec<String>, Vec<String>) =
            c.files.into_iter().partition(|f| files.contains(f));
        for u in unknown {
            info.warnings.push(format!(
                "claimed file `{u}` is not part of the diff; dropped"
            ));
        }
        changes.push(SubChange {
            summary: c.summary,
            files: known,
            symbols: c.symbols,
        });
    }
    let mut confidence = draft
        .confidence
        .unwrap_or(if info.method == "heuristic" { 0.3 } else { 0.7 })
        .clamp(0.0, 1.0);
    let unverified = symbols.iter().filter(|s| !s.verified).count();
    if unverified > 0 {
        confidence = (confidence - 0.1 * unverified as f64).max(0.1);
    }
    let kind = ChangeKind::parse_lenient(&draft.kind);
    let intent_id = make_intent_id(&[&draft.title, &draft.intent, &draft.behavior, &d.patch]);
    SemanticChange {
        schema: SCHEMA.into(),
        intent_id,
        kind,
        title: draft.title.trim().to_string(),
        intent: draft.intent.trim().to_string(),
        behavior: draft.behavior.trim().to_string(),
        changes,
        affected: Affected { files, symbols },
        constraints: draft.constraints,
        acceptance: draft.acceptance,
        tests: diff::tests_added_in_patch(&d.patch),
        dependencies: draft.dependencies,
        realization: Realization::default(),
        provenance: Provenance {
            operation: operation.into(),
            analysis: info,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ..Default::default()
        },
        confidence,
    }
}

pub fn make_intent_id(parts: &[&str]) -> String {
    let mut buf = String::new();
    for p in parts {
        buf.push_str(p);
        buf.push('\u{0}');
    }
    buf.push_str(
        &chrono::Utc::now()
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .to_string(),
    );
    let oid = git2::Oid::hash_object(git2::ObjectType::Blob, buf.as_bytes()).expect("hash");
    format!("int_{}", &oid.to_string()[..12])
}

/// Render a commit message from a record (title, body, trailer).
pub fn commit_message(rec: &SemanticChange, user_message: Option<&str>) -> String {
    let mut msg = match user_message {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => rec.title.clone(),
    };
    let mut body = vec![];
    if !rec.intent.is_empty() && !msg.contains(&rec.intent) {
        body.push(format!("Intent: {}", rec.intent));
    }
    if !rec.behavior.is_empty() {
        body.push(format!("Behavior: {}", rec.behavior));
    }
    if !body.is_empty() {
        msg.push_str("\n\n");
        msg.push_str(&body.join("\n"));
    }
    msg.push_str(&format!("\n\nGitent-Intent: {}\n", rec.intent_id));
    msg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{DiffInfo, FileChange};

    fn sample() -> DiffInfo {
        DiffInfo {
            patch: "--- a/u.py\n+++ b/u.py\n@@ -1 +1,3 @@\n+_C = {}\n def get_user(uid):\n-    return 1\n+    return _C.get(uid)\n".into(),
            files: vec![FileChange { path: "u.py".into(), old_path: None, status: "modified".into() }],
        }
    }

    #[test]
    fn claims_are_checked() {
        let draft = IntentDraft {
            title: "Cache".into(),
            intent: "speed".into(),
            symbols: vec![
                Symbol {
                    name: "_C".into(),
                    ..Default::default()
                },
                Symbol {
                    name: "ghost".into(),
                    ..Default::default()
                },
            ],
            changes: vec![SubChange {
                summary: "s".into(),
                files: vec!["u.py".into(), "nope.py".into()],
                symbols: vec![],
            }],
            confidence: Some(0.9),
            ..Default::default()
        };
        let rec = build_record(
            Analysis {
                draft,
                info: AnalysisInfo {
                    method: "llm".into(),
                    ..Default::default()
                },
            },
            &sample(),
            "commit",
        );
        let ghost = rec
            .affected
            .symbols
            .iter()
            .find(|s| s.name == "ghost")
            .unwrap();
        assert!(!ghost.verified);
        assert!(
            rec.affected
                .symbols
                .iter()
                .find(|s| s.name == "_C")
                .unwrap()
                .verified
        );
        assert_eq!(rec.changes[0].files, vec!["u.py".to_string()]);
        assert!(rec
            .provenance
            .analysis
            .warnings
            .iter()
            .any(|w| w.contains("nope.py")));
        assert!(rec.confidence < 0.9);
        assert!(rec.intent_id.starts_with("int_") && rec.intent_id.len() == 16);
    }

    #[test]
    fn heuristic_uses_conventional_prefix() {
        let a = heuristic(
            "fix(api): handle empty ids\n\nBecause crash.",
            &sample(),
            vec![],
        );
        assert_eq!(a.draft.kind, "fix");
        assert_eq!(a.draft.title, "handle empty ids");
        assert_eq!(a.draft.intent, "Because crash.");
    }

    #[test]
    fn normalizes_llm_shapes() {
        let v = serde_json::json!({"title":"t","intent":"i","constraints":"one","symbols":["a"],"confidence":"0.8"});
        let d: IntentDraft = serde_json::from_value(normalize_draft(v)).unwrap();
        assert_eq!(d.constraints, vec!["one"]);
        assert_eq!(d.symbols[0].name, "a");
        assert_eq!(d.confidence, Some(0.8));
    }
}
