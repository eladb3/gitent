//! Validation of a candidate result (ADR 0006).

use crate::config::ValidationConfig;
use crate::diff;
use crate::llm::{extract_json, Message, Provider};
use crate::model::{CheckResult, ValidationReport};
use crate::repo::DiffInfo;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

pub struct Ctx<'a> {
    pub worktree: &'a Path,
    /// Diff of the candidate against the step's starting tip.
    pub diff: &'a DiffInfo,
    /// Tests present at the starting tip.
    pub tests_before: &'a BTreeSet<String>,
    /// Tests the source change introduced and the result must contain.
    pub required_tests: &'a [String],
    /// Tests that may legitimately disappear (e.g. when reverting).
    pub removable_tests: &'a [String],
    pub cfg: &'a ValidationConfig,
    pub verifier: Option<&'a dyn Provider>,
    /// Human/LLM-readable description of what the step must achieve.
    pub goal: &'a str,
}

const VERIFIER_SYSTEM: &str = r#"You are Gitent's verifier. You judge whether a code change realises a stated
intention, without breaking stated constraints. Be skeptical: missing behaviour, behaviour applied twice,
weakened tests, or unrelated edits are failures. Respond with ONE JSON object only:
{"satisfied": true|false, "confidence": 0.0-1.0, "concerns": ["specific problem", ...]}"#;

pub fn validate(ctx: &Ctx) -> ValidationReport {
    let mut checks = vec![];
    let mut warnings = vec![];

    checks.push(CheckResult {
        name: "non_empty".into(),
        passed: !ctx.diff.files.is_empty(),
        detail: if ctx.diff.files.is_empty() {
            "the result is identical to the starting point".into()
        } else {
            String::new()
        },
    });

    let now = diff::tests_in_tree(ctx.worktree);
    let missing: Vec<&String> = ctx
        .required_tests
        .iter()
        .filter(|t| !now.contains(*t))
        .collect();
    checks.push(CheckResult {
        name: "source_tests_present".into(),
        passed: missing.is_empty(),
        detail: if missing.is_empty() {
            format!(
                "{} test(s) from the source change present",
                ctx.required_tests.len()
            )
        } else {
            format!(
                "tests introduced by the source change are missing: {}",
                join(&missing)
            )
        },
    });

    let lost: Vec<&String> = ctx
        .tests_before
        .iter()
        .filter(|t| !now.contains(*t) && !ctx.removable_tests.contains(t))
        .collect();
    checks.push(CheckResult {
        name: "existing_tests_preserved".into(),
        passed: lost.is_empty(),
        detail: if lost.is_empty() {
            String::new()
        } else {
            format!(
                "pre-existing tests were removed or renamed: {}",
                join(&lost)
            )
        },
    });

    let mut test_summary = String::from("no test command configured");
    if ctx.cfg.test_command.trim().is_empty() {
        warnings.push("no_test_command: behaviour was not validated by tests".into());
    } else {
        match crate::proc::run_shell(
            &ctx.cfg.test_command,
            ctx.worktree,
            None,
            &[],
            Duration::from_secs(ctx.cfg.timeout_secs),
        ) {
            Ok(o) => {
                let passed = o.success();
                test_summary = format!(
                    "`{}` {} in {} ms{}",
                    ctx.cfg.test_command,
                    if passed { "passed" } else { "FAILED" },
                    o.duration_ms,
                    if o.timed_out { " (timed out)" } else { "" }
                );
                checks.push(CheckResult {
                    name: "tests".into(),
                    passed,
                    detail: if passed {
                        test_summary.clone()
                    } else {
                        format!("{test_summary}\n{}", o.combined_tail(4000))
                    },
                });
            }
            Err(e) => checks.push(CheckResult {
                name: "tests".into(),
                passed: false,
                detail: format!("could not run tests: {e:#}"),
            }),
        }
    }

    let deterministic_ok = checks.iter().all(|c| c.passed);
    if deterministic_ok && ctx.cfg.verifier {
        match ctx.verifier {
            Some(p) => match verify(p, ctx, &test_summary) {
                Ok((sat, conf, concerns)) => {
                    let fail = !sat && conf >= ctx.cfg.verifier_threshold;
                    if !sat && !fail {
                        warnings.push(format!(
                            "verifier had low-confidence concerns ({conf:.2}): {}",
                            concerns.join("; ")
                        ));
                    }
                    checks.push(CheckResult {
                        name: "verifier".into(),
                        passed: !fail,
                        detail: format!(
                            "satisfied={sat} confidence={conf:.2}{}",
                            if concerns.is_empty() {
                                String::new()
                            } else {
                                format!(" concerns: {}", concerns.join("; "))
                            }
                        ),
                    });
                }
                Err(e) => {
                    if ctx.cfg.require_verifier {
                        checks.push(CheckResult {
                            name: "verifier".into(),
                            passed: false,
                            detail: format!("verifier failed: {e:#}"),
                        });
                    } else {
                        warnings.push(format!("verifier unavailable: {e:#}"));
                    }
                }
            },
            None if ctx.cfg.require_verifier => checks.push(CheckResult {
                name: "verifier".into(),
                passed: false,
                detail: "validation.require_verifier is set but no LLM provider is configured"
                    .into(),
            }),
            None => {}
        }
    }

    ValidationReport {
        passed: checks.iter().all(|c| c.passed),
        checks,
        warnings,
    }
}

fn verify(
    p: &dyn Provider,
    ctx: &Ctx,
    test_summary: &str,
) -> anyhow::Result<(bool, f64, Vec<String>)> {
    let patch = crate::proc::tail(&ctx.diff.patch, 40_000);
    let prompt = format!("## Goal\n{}\n\n## Test result\n{test_summary}\n\n## Resulting change (diff against the starting point)\n```diff\n{patch}```", ctx.goal);
    let v = extract_json(&p.complete(VERIFIER_SYSTEM, &[Message::user(prompt)])?)?;
    let sat = v["satisfied"].as_bool().unwrap_or(false);
    let conf = v["confidence"].as_f64().unwrap_or(0.5);
    let concerns = v["concerns"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok((sat, conf, concerns))
}

fn join(v: &[&String]) -> String {
    v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

/// Render failures as feedback for the next agent attempt.
pub fn feedback(r: &ValidationReport) -> String {
    let mut s = String::new();
    for c in r.failures() {
        s.push_str(&format!("- check `{}` failed: {}\n", c.name, c.detail));
    }
    s
}
