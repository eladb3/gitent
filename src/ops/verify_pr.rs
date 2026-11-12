//! `gitent verify-pr`: find the backports in a range of commits and verify them (ADR 0012).

use super::verify::{verify, VerifyOptions};
use super::Session;
use crate::repo::short;
use anyhow::{bail, Result};
use git2::Oid;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub const MARKER: &str = "<!-- gitent-verify-pr -->";

#[derive(Debug, Clone, Default)]
pub struct PrOptions {
    pub base: String,
    pub head: Option<String>,
    pub source: Option<String>,
    pub kind: Option<String>,
    pub require_source: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Detected {
    pub commit: String,
    pub source: String,
    pub kind: &'static str,
    pub marker: &'static str,
}

fn patterns() -> &'static [(Regex, &'static str, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            (
                Regex::new(r"(?m)^Gitent-Derived-From:\s*([0-9a-f]{7,40})").unwrap(),
                "apply",
                "Gitent-Derived-From",
            ),
            (
                Regex::new(r"\(cherry picked from commit ([0-9a-f]{7,40})\)").unwrap(),
                "apply",
                "cherry picked from",
            ),
            (
                Regex::new(r"(?i)\bBackport(?:ed)? (?:of|from) (?:commit )?([0-9a-f]{7,40})\b")
                    .unwrap(),
                "apply",
                "Backport of",
            ),
            (
                Regex::new(r"This reverts commit ([0-9a-f]{7,40})").unwrap(),
                "revert",
                "This reverts commit",
            ),
        ]
    })
}

/// Markers in a commit message, in priority order (first match wins).
pub fn detect(message: &str) -> Option<(String, &'static str, &'static str)> {
    detect_all(message).into_iter().next()
}

/// Every marker in a commit message (squashed PRs carry several), de-duplicated by sha.
pub fn detect_all(message: &str) -> Vec<(String, &'static str, &'static str)> {
    let mut out: Vec<(String, &'static str, &'static str)> = vec![];
    for (re, kind, name) in patterns() {
        for c in re.captures_iter(message) {
            let sha = c[1].to_string();
            if !out.iter().any(|(s, _, _)| s == &sha) {
                out.push((sha, kind, name));
            }
        }
    }
    out
}

pub const VERIFY_PR_SCHEMA: &str = "gitent/verify-pr@1";

pub fn verify_pr(s: &Session, o: &PrOptions) -> Result<Value> {
    let repo = &s.repo;
    let head = repo.resolve(o.head.as_deref().unwrap_or("HEAD"))?;
    let base_tip = repo.resolve(&o.base)?;
    let onto = repo.merge_base(base_tip, head)?;
    let commits = repo.commits_between(onto, head)?;

    // What does the PR move?
    let mut found: Vec<Detected> = vec![];
    let mut missing = vec![];
    if let Some(src) = &o.source {
        let oid = crate::store::resolve(repo, src)?;
        let kind = match o.kind.as_deref() {
            Some("revert") => "revert",
            Some("merge") => "merge",
            _ => "apply",
        };
        found.push(Detected {
            commit: head.to_string(),
            source: oid.to_string(),
            kind,
            marker: "--source",
        });
    } else {
        for c in &commits {
            let msg = repo
                .git
                .find_commit(*c)?
                .message()
                .unwrap_or("")
                .to_string();
            for (sha, kind, marker) in detect_all(&msg) {
                match repo.resolve(&sha) {
                    Ok(src) => found.push(Detected {
                        commit: c.to_string(),
                        source: src.to_string(),
                        kind: o
                            .kind
                            .as_deref()
                            .map(|k| if k == "revert" { "revert" } else { "apply" })
                            .unwrap_or(kind),
                        marker,
                    }),
                    Err(_) => {
                        missing.push(format!("{} (referenced by {})", sha, short(&c.to_string())))
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        bail!(
            "original commit(s) not found in this clone: {} — fetch full history (actions/checkout with fetch-depth: 0)",
            missing.join(", ")
        );
    }

    // A PR cannot cite its own commits as the change it moves (ADR 0013 §7).
    let own: Vec<String> = found
        .iter()
        .filter(|d| commits.iter().any(|c| c.to_string() == d.source))
        .map(|d| short(&d.source))
        .collect();
    let mut base_json = json!({
        "schema": VERIFY_PR_SCHEMA,
        "base": base_tip.to_string(), "onto": onto.to_string(), "head": head.to_string(),
        "commits": commits.len(), "detected": found,
    });
    // ...nor a commit that is already on the base branch: there is nothing to move (ADR 0015 §6).
    let on_base: Vec<String> = found
        .iter()
        .filter(|d| d.kind == "apply") // a revert's subject is on the base by definition
        .filter(|d| {
            Oid::from_str(&d.source)
                .map(|o| {
                    o == base_tip || repo.git.graph_descendant_of(base_tip, o).unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|d| short(&d.source))
        .collect();
    if own.is_empty() && !on_base.is_empty() {
        base_json["result"] = json!("rejected");
        base_json["state"] = json!("failed");
        base_json["verifications"] = json!([]);
        base_json["note"] = json!(format!(
            "the cited original commit(s) {} are already on the base branch; a backport must bring a change the base does not have",
            on_base.join(", ")
        ));
        base_json["error"] = json!("source commit is already on the base branch");
        base_json["markdown"] = json!(markdown(&base_json));
        return Ok(base_json);
    }
    if !own.is_empty() {
        base_json["result"] = json!("rejected");
        base_json["state"] = json!("failed");
        base_json["verifications"] = json!([]);
        base_json["note"] = json!(format!(
            "the PR cites its own commit(s) {} as the original change; a backport must reference a commit outside the PR",
            own.join(", ")
        ));
        base_json["error"] = json!("source commit is part of this PR");
        base_json["markdown"] = json!(markdown(&base_json));
        return Ok(base_json);
    }
    if found.is_empty() {
        let (result, state) = if o.require_source {
            ("rejected", "failed")
        } else {
            ("neutral", "succeeded")
        };
        base_json["result"] = json!(result);
        base_json["state"] = json!(state);
        base_json["verifications"] = json!([]);
        base_json["note"] = json!("no backport marker found (cherry picked from / Backport of / Gitent-Derived-From / This reverts commit); nothing to verify");
        if o.require_source {
            base_json["error"] =
                json!("--require-source: this PR does not reference the change it moves");
        }
        base_json["markdown"] = json!(markdown(&base_json));
        return Ok(base_json);
    }

    // Every source is verified against the whole PR (result = head, onto = merge base), so a
    // later commit in the PR that breaks the port is seen (ADR 0013 §7).
    let mut uniq: Vec<&Detected> = vec![];
    for d in &found {
        if !uniq.iter().any(|u| u.source == d.source) {
            uniq.push(d);
        }
    }
    let mut verifications = vec![];
    let mut verdicts = vec![];
    for d in uniq {
        let mut v = crate::store::without_persisting(|| {
            verify(
                s,
                &VerifyOptions {
                    source: d.source.clone(),
                    result: Some(head.to_string()),
                    onto: Some(onto.to_string()),
                    working_tree: false,
                    kind: d.kind.to_string(),
                    record: false,
                },
            )
        })?;
        verdicts.push(v["verdict"].as_str().unwrap_or("rejected").to_string());
        let subj = repo
            .git
            .find_commit(Oid::from_str(&d.source)?)?
            .summary()
            .unwrap_or("")
            .to_string();
        v["source_subject"] = json!(subj);
        v["marker"] = json!(d.marker);
        verifications.push(v);
    }
    let result = if verdicts.iter().any(|v| v == "rejected") {
        "rejected"
    } else if verdicts.iter().any(|v| v == "unverified") {
        "unverified"
    } else {
        "verified"
    };
    base_json["result"] = json!(result);
    base_json["state"] = json!(if result == "verified" {
        "succeeded"
    } else {
        "failed"
    });
    base_json["verifications"] = json!(verifications);
    match result {
        "rejected" => base_json["error"] = json!("backport verification failed (see the report)"),
        "unverified" => {
            base_json["error"] = json!("backport not proven: needs human review (see the report)")
        }
        _ => {}
    }
    base_json["markdown"] = json!(markdown(&base_json));
    Ok(base_json)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// The PR comment / step summary.
pub fn markdown(v: &Value) -> String {
    let mut m = format!("{MARKER}\n");
    let (icon, title) = match s(v, "result") {
        "verified" => ("✅", "backport verified"),
        "rejected" => ("❌", "backport rejected"),
        "unverified" => ("⚠️", "backport not proven (needs review)"),
        _ => ("➖", "nothing to verify"),
    };
    m.push_str(&format!("### {icon} Gitent: {title}\n\n"));
    if let Some(n) = v.get("note").and_then(|n| n.as_str()) {
        m.push_str(&format!("{n}.\n"));
        if let Some(e) = v.get("error").and_then(|n| n.as_str()) {
            m.push_str(&format!("\n**{e}**\n"));
        }
        return m;
    }
    for x in v["verifications"].as_array().into_iter().flatten() {
        let verdict = s(x, "verdict");
        m.push_str(&format!(
            "**{}** {} `{}` {}: {} onto `{}` → `{}`\n\n",
            match verdict {
                "verified" => "✅",
                "unverified" => "⚠️",
                _ => "❌",
            },
            s(x, "kind"),
            short(s(x, "source")),
            md_inline(s(x, "source_subject")),
            if verdict.is_empty() {
                "rejected"
            } else {
                verdict
            },
            short(s(x, "base")),
            short(s(x, "result"))
        ));
        m.push_str("| check | result |\n|---|---|\n");
        let mut failures = vec![];
        for c in x["validation"]["checks"].as_array().into_iter().flatten() {
            let passed = c["passed"] == true;
            let detail = s(c, "detail");
            let first = detail.lines().next().unwrap_or("").to_string();
            m.push_str(&format!(
                "| {} `{}` | {} |\n",
                if passed {
                    "✓"
                } else if c["review"] == true {
                    "?"
                } else {
                    "✗"
                },
                s(c, "name"),
                md_inline(&crate::proc::tail(&first, 160))
            ));
            if !passed && detail.lines().count() > 1 {
                failures.push((s(c, "name").to_string(), crate::proc::tail(detail, 3000)));
            }
        }
        for w in x["validation"]["warnings"].as_array().into_iter().flatten() {
            m.push_str(&format!(
                "\n> ⚠️ {}\n",
                md_inline(&crate::proc::tail(w.as_str().unwrap_or(""), 500))
            ));
        }
        for (name, detail) in failures {
            let fence = fence_for(&detail);
            m.push_str(&format!(
                "\n<details><summary>{name}: details</summary>\n\n{fence}\n{detail}\n{fence}\n</details>\n"
            ));
        }
        m.push('\n');
    }
    m.push_str("<sub>Checked by [gitent verify](https://github.com/eladb3/gitent/blob/main/docs/GITHUB_ACTION.md): the original change's requirements, the target's tests and requirements, no resurrected files, no dropped definitions, no deleted, skipped or weakened tests. ⚠️ means nothing is known to be wrong but the backport is not proven, so a human should look.</sub>\n");
    m
}

/// Text from commits/tests going into a Markdown line: no HTML, no code spans, no table breaks.
pub(crate) fn md_inline(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for ch in t.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '`' => out.push('\''),
            '@' => out.push_str("&#64;"), // no mentions
            '\n' | '\r' => out.push(' '),
            '|' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '!' | '~' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// A code fence longer than any backtick run in `t`, so `t` cannot close it.
/// Text going inside a `code span` in a table cell: escapes inside code spans are shown
/// literally, so only what could end the span or the cell is replaced.
pub(crate) fn code_inline(t: &str) -> String {
    t.chars()
        .filter(|c| !c.is_control() || *c == ' ')
        .map(|c| match c {
            '`' => "'".to_string(),
            '|' => "\\|".to_string(),
            '\n' | '\r' => " ".to_string(),
            c => c.to_string(),
        })
        .collect()
}

pub(crate) fn fence_for(t: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in t.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::{detect, md_inline};

    #[test]
    fn md_inline_neutralises_markup() {
        let s = md_inline("x](http://evil) <img> @team `code` | *b*\nnext");
        assert!(
            !s.contains("](") && !s.contains('<') && !s.contains('@') && !s.contains('`'),
            "{s}"
        );
        assert!(!s.contains('\n'));
    }

    #[test]
    fn detects_markers() {
        assert_eq!(
            detect("fix\n\n(cherry picked from commit 0123456789abcdef)")
                .unwrap()
                .0,
            "0123456789abcdef"
        );
        assert_eq!(
            detect("[5.2.x] Fixed #1 -- x.\n\nBackport of 7ff7fcc0508864a from main.")
                .unwrap()
                .0,
            "7ff7fcc0508864a"
        );
        assert_eq!(
            detect("x\n\nGitent-Intent: int_1\nGitent-Derived-From: abcdef1234567\n")
                .unwrap()
                .2,
            "Gitent-Derived-From"
        );
        let (sha, kind, _) = detect("Revert \"x\"\n\nThis reverts commit 89abcdef0123.").unwrap();
        assert_eq!((sha.as_str(), kind), ("89abcdef0123", "revert"));
        assert!(detect("just a normal commit, backport of nothing").is_none());
    }
}
