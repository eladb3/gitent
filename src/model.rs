//! Semantic data model. These types are part of the public JSON API.

use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "gitent/semantic-change@2";
pub const SCHEMA_V1: &str = "gitent/semantic-change@1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    #[default]
    Feature,
    Fix,
    Refactor,
    Perf,
    Test,
    Docs,
    Chore,
    Revert,
    Merge,
}

impl ChangeKind {
    pub fn parse_lenient(s: &str) -> ChangeKind {
        match s.trim().to_ascii_lowercase().as_str() {
            "fix" | "bugfix" | "bug" => ChangeKind::Fix,
            "refactor" | "refactoring" => ChangeKind::Refactor,
            "perf" | "performance" => ChangeKind::Perf,
            "test" | "tests" => ChangeKind::Test,
            "docs" | "doc" | "documentation" => ChangeKind::Docs,
            "chore" | "build" | "ci" => ChangeKind::Chore,
            "revert" => ChangeKind::Revert,
            "merge" => ChangeKind::Merge,
            _ => ChangeKind::Feature,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Symbol {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// True if the symbol name literally occurs in the diff.
    #[serde(default = "yes")]
    pub verified: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SubChange {
    pub summary: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Affected {
    pub files: Vec<String>,
    pub symbols: Vec<Symbol>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Realization {
    pub commit: String,
    pub parents: Vec<String>,
    pub tree: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AnalysisInfo {
    /// "llm", "host", or "heuristic"
    pub method: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceRef {
    pub commit: String,
    pub intent_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Provenance {
    /// commit | cherry-pick | merge | rebase | revert | annotate
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<SourceRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverts: Option<SourceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub composes: Vec<SourceRef>,
    /// "git" when the deterministic path succeeded, otherwise executor name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub executor: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub op_id: String,
    pub analysis: AnalysisInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<ValidationReport>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SemanticChange {
    pub schema: String,
    pub intent_id: String,
    pub kind: ChangeKind,
    pub title: String,
    pub intent: String,
    pub behavior: String,
    #[serde(default)]
    pub changes: Vec<SubChange>,
    pub affected: Affected,
    /// Checkable statements this change must satisfy (ADR 0008).
    #[serde(default)]
    pub requirements: Vec<Requirement>,
    /// Requirements this change deliberately ends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retires: Vec<Retirement>,
    /// `@1` only: read and migrated into `requirements` by [`SemanticChange::migrate`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    /// `@1` only: read and migrated into `requirements`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub tests: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub realization: Realization,
    pub provenance: Provenance,
    pub confidence: f64,
}

/// The LLM- or host-supplied part of a semantic change (before deterministic
/// fields are filled in). Also the input format of `--intent-file`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct IntentDraft {
    pub title: String,
    pub kind: String,
    pub intent: String,
    pub behavior: String,
    pub changes: Vec<SubChange>,
    pub symbols: Vec<Symbol>,
    pub constraints: Vec<String>,
    pub acceptance: Vec<String>,
    pub dependencies: Vec<String>,
    pub requirements: Vec<RequirementDraft>,
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ReqKind {
    Test,
    Command,
    #[default]
    Judged,
}

impl ReqKind {
    pub fn executable(self) -> bool {
        !matches!(self, ReqKind::Judged)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckSpec {
    pub run: String,
    #[serde(default)]
    pub expect_exit: i32,
    #[serde(default = "default_check_timeout")]
    pub timeout_secs: u64,
}

fn default_check_timeout() -> u64 {
    120
}

/// Evidence gathered when a requirement was proven (ADR 0008 §4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Proof {
    /// pass | fail | skipped
    pub before: String,
    pub after: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Requirement {
    pub id: String,
    pub statement: String,
    /// behavior | constraint
    #[serde(default = "behavior")]
    pub category: String,
    pub kind: ReqKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub test: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckSpec>,
    /// auto | author | llm | migrated
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<Proof>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebound_from: Option<String>,
}

fn behavior() -> String {
    "behavior".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Retirement {
    pub id: String,
    pub reason: String,
}

/// Requirement input (author flags, intent drafts, LLM proposals).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct RequirementDraft {
    pub statement: String,
    pub category: String,
    /// test | command | judged (inferred from `test`/`check` when empty)
    pub kind: String,
    pub test: String,
    pub check: Option<CheckSpec>,
}

impl SemanticChange {
    /// Upgrade an `@1` record in memory (ADR 0008 consequences).
    pub fn migrate(mut self) -> SemanticChange {
        if self.schema == SCHEMA_V1 {
            let mut n = 0;
            let mut id = |s: &str| {
                n += 1;
                format!(
                    "req_{}",
                    crate::analyze::short_hash(&[&self.intent_id, s, &n.to_string()])
                )
            };
            let mut reqs = vec![];
            for t in &self.tests {
                reqs.push(Requirement {
                    id: id(t),
                    statement: humanize_test(t),
                    category: "behavior".into(),
                    kind: ReqKind::Test,
                    test: t.clone(),
                    origin: "migrated".into(),
                    ..Default::default()
                });
            }
            for a in &self.acceptance {
                reqs.push(Requirement {
                    id: id(a),
                    statement: a.clone(),
                    category: "behavior".into(),
                    kind: ReqKind::Judged,
                    origin: "migrated".into(),
                    ..Default::default()
                });
            }
            for c in &self.constraints {
                reqs.push(Requirement {
                    id: id(c),
                    statement: c.clone(),
                    category: "constraint".into(),
                    kind: ReqKind::Judged,
                    origin: "migrated".into(),
                    ..Default::default()
                });
            }
            self.requirements = reqs;
            self.acceptance.clear();
            self.constraints.clear();
            self.schema = SCHEMA.into();
        }
        self
    }

    pub fn statements(&self, category: &str) -> Vec<String> {
        self.requirements
            .iter()
            .filter(|r| r.category == category)
            .map(|r| r.statement.clone())
            .collect()
    }
}

/// `test_cache_hit` → "cache hit"; `TestCacheHit` → "cache hit".
pub fn humanize_test(t: &str) -> String {
    let t = t
        .strip_prefix("test_")
        .or_else(|| t.strip_prefix("Test"))
        .unwrap_or(t);
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if ch == '_' {
            out.push(' ');
        } else if ch.is_uppercase() && i > 0 {
            out.push(' ');
            out.extend(ch.to_lowercase());
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out.trim().to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ValidationReport {
    pub passed: bool,
    pub checks: Vec<CheckResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn failures(&self) -> Vec<&CheckResult> {
        self.checks.iter().filter(|c| !c.passed).collect()
    }
}

/// JSON schema (hand-written, kept in sync with the structs above) for `gitent schema`.
pub fn json_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SCHEMA,
        "title": "SemanticChange",
        "type": "object",
        "required": ["schema","intent_id","kind","title","intent","behavior","affected","realization","provenance","confidence"],
        "properties": {
            "schema": {"const": SCHEMA},
            "intent_id": {"type":"string","pattern":"^int_[0-9a-f]{12}$","description":"Identity of the intention, shared by all realisations"},
            "kind": {"enum":["feature","fix","refactor","perf","test","docs","chore","revert","merge"]},
            "title": {"type":"string"},
            "intent": {"type":"string","description":"Why the change exists"},
            "behavior": {"type":"string","description":"Observable behaviour after the change"},
            "changes": {"type":"array","items":{"type":"object","properties":{"summary":{"type":"string"},"files":{"type":"array","items":{"type":"string"}},"symbols":{"type":"array","items":{"type":"string"}}}}},
            "affected": {"type":"object","properties":{
                "files":{"type":"array","items":{"type":"string"}},
                "symbols":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"kind":{"type":"string"},"file":{"type":"string"},"verified":{"type":"boolean"}}}}}},
            "requirements": {"type":"array","items":{"$ref":"#/$defs/requirement"}},
            "retires": {"type":"array","items":{"type":"object","required":["id","reason"],"properties":{"id":{"type":"string"},"reason":{"type":"string"}}}},
            "tests": {"type":"array","items":{"type":"string"},"description":"Test identifiers introduced or modified by the change"},
            "dependencies": {"type":"array","items":{"type":"string"}},
            "realization": {"type":"object","properties":{"commit":{"type":"string"},"parents":{"type":"array","items":{"type":"string"}},"tree":{"type":"string"}}},
            "provenance": {"type":"object","properties":{
                "operation":{"enum":["commit","cherry-pick","merge","rebase","revert","annotate"]},
                "derived_from":{"$ref":"#/$defs/source"},
                "reverts":{"$ref":"#/$defs/source"},
                "composes":{"type":"array","items":{"$ref":"#/$defs/source"}},
                "executor":{"type":"string"},
                "attempts":{"type":"integer"},
                "op_id":{"type":"string"},
                "analysis":{"type":"object","properties":{"method":{"enum":["llm","host","heuristic"]},"provider":{"type":"string"},"warnings":{"type":"array","items":{"type":"string"}}}},
                "validation":{"$ref":"#/$defs/validation"},
                "created_at":{"type":"string","format":"date-time"}}},
            "confidence": {"type":"number","minimum":0,"maximum":1}
        },
        "$defs": {
            "source": {"type":"object","properties":{"commit":{"type":"string"},"intent_id":{"type":"string"}}},
            "requirement": {"type":"object","required":["id","statement","kind","origin"],"properties":{
                "id":{"type":"string","pattern":"^req_[0-9a-f]{8}$"},
                "statement":{"type":"string"},
                "category":{"enum":["behavior","constraint"]},
                "kind":{"enum":["test","command","judged"]},
                "test":{"type":"string"},
                "check":{"type":"object","properties":{"run":{"type":"string"},"expect_exit":{"type":"integer"},"timeout_secs":{"type":"integer"}}},
                "origin":{"enum":["auto","author","llm","migrated"]},
                "proof":{"type":"object","properties":{"before":{"enum":["pass","fail","skipped"]},"after":{"enum":["pass","fail","skipped"]},"commit":{"type":"string"}}},
                "rebound_from":{"type":"string"}}},
            "validation": {"type":"object","properties":{"passed":{"type":"boolean"},
                "checks":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"passed":{"type":"boolean"},"detail":{"type":"string"}}}},
                "warnings":{"type":"array","items":{"type":"string"}}}},
            "intent_draft": {
                "description":"Input accepted by `gitent commit --intent-file` and the MCP `intent` argument",
                "type":"object","required":["title","intent"],
                "properties":{"title":{"type":"string"},"kind":{"type":"string"},"intent":{"type":"string"},"behavior":{"type":"string"},
                    "changes":{"type":"array"},"symbols":{"type":"array"},"constraints":{"type":"array","items":{"type":"string"}},
                    "acceptance":{"type":"array","items":{"type":"string"}},"dependencies":{"type":"array","items":{"type":"string"}},
                    "requirements":{"type":"array","items":{"type":"object","properties":{"statement":{"type":"string"},"category":{"type":"string"},"kind":{"type":"string"},"test":{"type":"string"},"check":{"type":"object"}}}},
                    "confidence":{"type":"number"}}}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_parsing_is_lenient() {
        assert_eq!(ChangeKind::parse_lenient("Bugfix"), ChangeKind::Fix);
        assert_eq!(ChangeKind::parse_lenient("whatever"), ChangeKind::Feature);
    }

    #[test]
    fn draft_accepts_partial_json() {
        let d: IntentDraft = serde_json::from_str(r#"{"title":"x","intent":"y"}"#).unwrap();
        assert_eq!(d.title, "x");
        assert!(d.symbols.is_empty());
    }

    #[test]
    fn migrates_v1_records() {
        let v1 = serde_json::json!({
            "schema": SCHEMA_V1, "intent_id": "int_000000000001", "kind": "feature", "title": "t", "intent": "i",
            "behavior": "b", "affected": {"files": [], "symbols": []}, "constraints": ["keep sig"],
            "acceptance": ["is cached"], "tests": ["test_cache_hit"], "realization": {"commit": "", "parents": [], "tree": ""},
            "provenance": {"operation": "commit", "analysis": {"method": "llm"}, "created_at": ""}, "confidence": 0.5
        });
        let r: SemanticChange = serde_json::from_value(v1).unwrap();
        let r = r.migrate();
        assert_eq!(r.schema, SCHEMA);
        assert_eq!(r.requirements.len(), 3);
        assert_eq!(r.requirements[0].kind, ReqKind::Test);
        assert_eq!(r.requirements[0].statement, "cache hit");
        assert_eq!(r.requirements[2].category, "constraint");
        assert!(r.acceptance.is_empty() && r.constraints.is_empty());
    }

    #[test]
    fn humanizes_test_names() {
        assert_eq!(humanize_test("test_cache_hit"), "cache hit");
        assert_eq!(humanize_test("TestCacheHit"), "cache hit");
    }

    #[test]
    fn symbol_verified_defaults_true() {
        let s: Symbol = serde_json::from_str(r#"{"name":"f"}"#).unwrap();
        assert!(s.verified);
    }
}
