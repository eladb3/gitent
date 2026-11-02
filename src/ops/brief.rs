//! Task briefs handed to executors (and host agents).

use crate::diff;
use crate::model::SemanticChange;
use std::path::Path;

pub struct BriefInput<'a> {
    pub op_kind: &'a str,
    pub step_no: usize,
    pub step_count: usize,
    pub worktree: &'a Path,
    pub goal: &'a str,
    /// Records driving this step (source intention, or theirs' intentions for merge).
    pub records: &'a [SemanticChange],
    /// Records already present on our side (merge only) that must be preserved.
    pub preserve: &'a [SemanticChange],
    /// Reference diff (original change, or theirs since merge base).
    pub reference_diff: &'a str,
    pub reference_label: &'a str,
    pub starting_point: &'a str,
    pub required_tests: &'a [String],
    pub removable_tests: &'a [String],
    pub test_command: &'a str,
    pub feedback: &'a [String],
}

pub const MAX_REF_DIFF: usize = 30_000;

pub fn render(b: &BriefInput) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "# Gitent task — {} (step {}/{})\n\n",
        b.op_kind, b.step_no, b.step_count
    ));
    s.push_str(&format!(
        "You are working in an isolated git worktree: `{}`.\n\
         Your current directory is that worktree. Edit files in place.\n\
         Do NOT commit, do NOT create branches, do NOT modify anything outside this directory.\n\
         Gitent will validate your result and commit it.\n\n",
        b.worktree.display()
    ));
    s.push_str("## Goal\n\n");
    s.push_str(b.goal.trim());
    s.push_str("\n\n");

    for (label, recs) in [
        ("Intention(s) to realise", b.records),
        (
            "Intention(s) already in the current code (must keep working)",
            b.preserve,
        ),
    ] {
        if recs.is_empty() {
            continue;
        }
        s.push_str(&format!("## {label}\n\n"));
        for r in recs {
            s.push_str(&format!(
                "### {} `{}` ({:?})\n",
                r.title, r.intent_id, r.kind
            ));
            if !r.intent.is_empty() {
                s.push_str(&format!("- **Why:** {}\n", r.intent));
            }
            if !r.behavior.is_empty() {
                s.push_str(&format!("- **Behaviour:** {}\n", r.behavior));
            }
            for a in &r.acceptance {
                s.push_str(&format!("- **Must hold:** {a}\n"));
            }
            for c in &r.constraints {
                s.push_str(&format!("- **Constraint:** {c}\n"));
            }
            for d in &r.dependencies {
                s.push_str(&format!("- **Depends on:** {d}\n"));
            }
            for c in &r.changes {
                s.push_str(&format!("- sub-change: {}\n", c.summary));
            }
            if !r.affected.symbols.is_empty() {
                let names: Vec<String> = r
                    .affected
                    .symbols
                    .iter()
                    .map(|x| format!("`{}`", x.name))
                    .collect();
                s.push_str(&format!(
                    "- symbols in the original code: {}\n",
                    names.join(", ")
                ));
            }
            s.push('\n');
        }
    }

    // Where do the original symbols live now?
    let mut names: Vec<String> = b
        .records
        .iter()
        .flat_map(|r| {
            r.affected
                .symbols
                .iter()
                .map(|x| x.name.clone())
                .chain(r.changes.iter().flat_map(|c| c.symbols.clone()))
        })
        .collect();
    names.sort();
    names.dedup();
    let locs = diff::locate(b.worktree, &names, 40);
    s.push_str("## Where the original symbols occur in the current code\n\n");
    if locs.is_empty() {
        s.push_str("None of the original symbol names occur in the current code — they were probably renamed or \
                    moved. Search for the equivalent functionality by behaviour.\n\n");
    } else {
        for l in &locs {
            s.push_str(&format!(
                "- `{}:{}` {}{}\n",
                l.file,
                l.line,
                if l.is_definition { "(definition) " } else { "" },
                l.text
            ));
        }
        s.push('\n');
    }

    s.push_str(&format!(
        "## Starting point\n\n{}\n\n",
        b.starting_point.trim()
    ));

    s.push_str(&format!(
        "## {} (reference only — the code has changed since; do not apply it blindly)\n\n",
        b.reference_label
    ));
    s.push_str("```diff\n");
    s.push_str(&crate::proc::tail(b.reference_diff, MAX_REF_DIFF));
    if !b.reference_diff.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n\n");

    s.push_str("## Requirements\n\n");
    s.push_str("- Realise the behaviour in the *current* code structure: adapt to renamed functions, moved files and changed signatures.\n");
    s.push_str(
        "- Keep the change minimal and focused on the goal; do not refactor unrelated code.\n",
    );
    if !b.required_tests.is_empty() {
        s.push_str(&format!(
            "- Port the tests from the reference change, adapted to the current code, **keeping these test names**: {}.\n",
            b.required_tests.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    if !b.removable_tests.is_empty() {
        s.push_str(&format!(
            "- These tests assert the behaviour being removed and may be deleted or inverted: {}.\n",
            b.removable_tests.iter().map(|t| format!("`{t}`")).collect::<Vec<_>>().join(", ")
        ));
    }
    s.push_str("- Do not delete, rename, skip or weaken any other existing test.\n");
    if b.test_command.trim().is_empty() {
        s.push_str("- No test command is configured; be especially careful.\n");
    } else {
        s.push_str(&format!(
            "- Gitent will validate by running `{}` in this directory. You may run it yourself.\n",
            b.test_command
        ));
    }

    if !b.feedback.is_empty() {
        s.push_str("\n## Feedback from previous attempts (fix these)\n\n");
        for (i, f) in b.feedback.iter().enumerate() {
            s.push_str(&format!("### Attempt {}\n{}\n", i + 1, f.trim()));
        }
    }
    s
}
