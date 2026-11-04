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
    for (re, kind, name) in patterns() {
        if let Some(c) = re.captures(message) {
            return Some((c[1].to_string(), kind, name));
        }
    }
    None
}

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
            if let Some((sha, kind, marker)) = detect(&msg) {
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

    let mut base_json = json!({
        "base": base_tip.to_string(), "onto": onto.to_string(), "head": head.to_string(),
        "commits": commits.len(), "detected": found,
    });
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

    // One source → verify the PR as a whole; several → each marked commit on its own parent.
    let mut uniq: Vec<&Detected> = vec![];
    for d in &found {
        if !uniq.iter().any(|u| u.source == d.source) {
            uniq.push(d);
        }
    }
    let mut runs = vec![];
    if uniq.len() == 1 {
        let d = uniq[0];
        runs.push((d.clone(), head, onto));
    } else {
        for d in &found {
            let c = Oid::from_str(&d.commit)?;
            let parent = repo.git.find_commit(c)?.parent_id(0)?;
            runs.push((d.clone(), c, parent));
        }
    }
    let mut verifications = vec![];
    let mut all_ok = true;
    for (d, result, base) in runs {
        let mut v = verify(
            s,
            &VerifyOptions {
                source: d.source.clone(),
                result: Some(result.to_string()),
                onto: Some(base.to_string()),
                working_tree: false,
                kind: d.kind.to_string(),
                record: false,
            },
        )?;
        all_ok &= v["verified"] == true;
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
    base_json["result"] = json!(if all_ok { "verified" } else { "rejected" });
    base_json["state"] = json!(if all_ok { "succeeded" } else { "failed" });
    base_json["verifications"] = json!(verifications);
    if !all_ok {
        base_json["error"] = json!("backport verification failed (see the report)");
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
        let ok = x["verified"] == true;
        m.push_str(&format!(
            "**{}** {} `{}` {}: {} onto `{}` → `{}`\n\n",
            if ok { "✅" } else { "❌" },
            s(x, "kind"),
            short(s(x, "source")),
            s(x, "source_subject"),
            if ok { "verified" } else { "rejected" },
            short(s(x, "base")),
            short(s(x, "result"))
        ));
        m.push_str("| check | result |\n|---|---|\n");
        let mut failures = vec![];
        for c in x["validation"]["checks"].as_array().into_iter().flatten() {
            let passed = c["passed"] == true;
            let detail = s(c, "detail");
            let first = detail.lines().next().unwrap_or("").replace('|', "\\|");
            m.push_str(&format!(
                "| {} `{}` | {} |\n",
                if passed { "✓" } else { "✗" },
                s(c, "name"),
                crate::proc::tail(&first, 160)
            ));
            if !passed && detail.lines().count() > 1 {
                failures.push((s(c, "name").to_string(), crate::proc::tail(detail, 3000)));
            }
        }
        for w in x["validation"]["warnings"].as_array().into_iter().flatten() {
            m.push_str(&format!("\n> ⚠️ {}\n", w.as_str().unwrap_or("")));
        }
        for (name, detail) in failures {
            m.push_str(&format!(
                "\n<details><summary>{name}: details</summary>\n\n```\n{detail}\n```\n</details>\n"
            ));
        }
        m.push('\n');
    }
    m.push_str("<sub>Checked by [gitent verify](https://github.com/eladb3/gitent/blob/main/docs/GITHUB_ACTION.md): the original change's requirements, the target's tests and requirements, no resurrected files, no dropped definitions, no deleted tests.</sub>\n");
    m
}

#[cfg(test)]
mod tests {
    use super::detect;

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
