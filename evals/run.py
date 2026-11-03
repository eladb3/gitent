#!/usr/bin/env python3
"""Comparative eval: plain git vs a baseline coding agent vs gitent.

Each case (evals/cases.py) builds a diverged history and names one operation.
Every arm starts from an identical, freshly built repository:

  git       the plain git operation (cherry-pick / merge / revert)
  baseline  the same coding agent, given the commit message(s) and diff, no gitent
  gitent    gitent, using the same agent as its `command` executor

Results are scored with HIDDEN tests that no arm ever sees.

  python3 evals/run.py --agent offline           # reference solutions: validates the harness
  python3 evals/run.py --agent claude-code       # real agent (costs tokens)

Stdlib only.
"""
import argparse
import concurrent.futures as cf
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
from cases import CASES  # noqa: E402

CLAUDE = ('claude -p --permission-mode acceptEdits --allowedTools '
          '"Bash(python3:*)" "Bash(python:*)" "Bash(git diff:*)" "Bash(git status:*)" "Bash(git log:*)" "Bash(git show:*)" "Bash(ls:*)"')
AGENTS = {"claude-code": CLAUDE, "claude-code-haiku": CLAUDE.replace("claude -p", "claude -p --model claude-haiku-5-5", 1)}
CLEAN_ENV = {k: v for k, v in os.environ.items() if not k.startswith("GITENT_")}
CLEAN_ENV.update(GIT_AUTHOR_NAME="Eval", GIT_AUTHOR_EMAIL="eval@example.com",
                 GIT_COMMITTER_NAME="Eval", GIT_COMMITTER_EMAIL="eval@example.com",
                 PYTHONDONTWRITEBYTECODE="1")


def sh(cmd, cwd, stdin=None, timeout=1800, env=None, check=False):
    p = subprocess.run(cmd, cwd=cwd, input=stdin, text=True, capture_output=True, shell=isinstance(cmd, str),
                       timeout=timeout, env=env or CLEAN_ENV)
    if check and p.returncode != 0:
        raise RuntimeError(f"{cmd} failed in {cwd}:\n{p.stdout}\n{p.stderr}")
    return p


def write_files(root, files):
    for rel, content in files.items():
        p = Path(root) / rel
        if content is None:
            if p.exists():
                p.unlink()
            continue
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(content)


def build(case, repo, gitent):
    repo.mkdir(parents=True)
    sh(["git", "init", "-q", "-b", "main"], repo, check=True)
    write_files(repo, case["base"])
    sh([gitent, "init"], repo, check=True)
    sh(["git", "add", "-A"], repo, check=True)
    sh(["git", "commit", "-qm", "base"], repo, check=True)
    for step in case["history"]:
        if step.get("from"):
            sh(["git", "checkout", "-qb", step["branch"], step["from"]], repo, check=True)
        else:
            sh(["git", "checkout", "-q", step["branch"]], repo, check=True)
        write_files(repo, step["files"])
        p = sh([gitent, "--json", "commit", "-m", step["message"]], repo)
        if p.returncode != 0:
            raise RuntimeError(f"setup commit failed for {case['id']}: {p.stdout}{p.stderr}")
        if step.get("tag"):
            sh(["git", "tag", step["tag"]], repo, check=True)
    sh(["git", "checkout", "-q", "main"], repo, check=True)


def baseline_prompt(case, repo):
    op, rev = case["operation"]["op"], case["operation"]["rev"]
    tests = "python3 -m unittest discover -q"
    if op == "rebase":
        br = case["operation"]["branch"]
        base = sh(["git", "merge-base", br, rev], repo).stdout.strip()
        commits = sh(["git", "rev-list", "--reverse", f"{base}..{br}"], repo).stdout.split()
        parts = []
        for c in commits:
            msg = sh(["git", "log", "-1", "--format=%B", c], repo).stdout
            diff = sh(["git", "show", "--format=", c], repo).stdout
            parts.append(f"Commit {c[:10]}:\n{msg}\n```diff\n{diff}```")
        task = (f"Apply, in order, the {len(commits)} commits of branch `{br}` on top of the current branch (`{rev}`), "
                f"which has changed since they were written; adapt as needed so ALL of their changes work on the current code.\n\n"
                + "\n\n".join(parts))
    elif op == "merge":
        base = sh(["git", "merge-base", "HEAD", rev], repo).stdout.strip()
        ours = sh(["git", "log", "--format=- %s%n%b", f"{base}..HEAD"], repo).stdout
        theirs = sh(["git", "log", "--format=- %s%n%b", f"{base}..{rev}"], repo).stdout
        diff = sh(["git", "diff", base, rev], repo).stdout
        task = (f"Merge branch `{rev}` into the current branch so that the changes of BOTH branches are preserved.\n\n"
                f"Commits on the current branch since the merge base:\n{ours}\nCommits on `{rev}`:\n{theirs}\n"
                f"Changes on `{rev}` since the merge base:\n```diff\n{diff}```")
    else:
        sha = sh(["git", "rev-parse", rev], repo).stdout.strip()
        msg = sh(["git", "log", "-1", "--format=%B", sha], repo).stdout
        diff = sh(["git", "show", "--format=", sha], repo).stdout
        verb = "Apply the change from" if op == "cherry-pick" else "Revert the change introduced by"
        task = (f"{verb} commit {sha[:10]} on the current branch. The current branch may have changed since that "
                f"commit was written; adapt as needed and keep all other behaviour working.\n\n"
                f"Commit message:\n{msg}\nCommit diff:\n```diff\n{diff}```")
    return (f"You are working in a git repository (the current directory), on branch main.\n\nTask: {task}\n\n"
            f"The project's tests run with: {tests}\nEdit files directly. Do not commit and do not change git history.\n"
            f"When you are done, print a final line `RESULT: SUCCESS` if the task is fully done, otherwise `RESULT: FAILURE`.\n")


def git_arm(case, repo):
    op, rev = case["operation"]["op"], case["operation"]["rev"]
    if op == "rebase":
        sh(["git", "checkout", "-q", case["operation"]["branch"]], repo, check=True)
    cmd = {"cherry-pick": ["git", "cherry-pick", rev], "merge": ["git", "merge", "--no-edit", rev],
           "revert": ["git", "revert", "--no-edit", rev], "rebase": ["git", "rebase", rev]}[op]
    p = sh(cmd, repo)
    ok = p.returncode == 0
    if not ok:
        sh(["git", op, "--abort"], repo)
    return {"reported_success": ok, "detail": (p.stdout + p.stderr)[-600:]}


def baseline_arm(case, repo, agent_cmd, timeout):
    p = sh(agent_cmd, repo, stdin=baseline_prompt(case, repo), timeout=timeout)
    out = p.stdout + p.stderr
    m = re.findall(r"RESULT:\s*(SUCCESS|FAILURE)", out)
    return {"reported_success": bool(m) and m[-1] == "SUCCESS", "detail": out[-1500:]}


def gitent_arm(case, repo, gitent, agent_cmd, llm_cmd, timeout):
    op, rev = case["operation"]["op"], case["operation"]["rev"]
    env = dict(CLEAN_ENV)
    args = [gitent, "--json", "--executor", "command", "--executor-command", agent_cmd]
    if llm_cmd:
        args += ["--provider", "command"]
        env["GITENT_LLM_COMMAND"] = llm_cmd
    if op == "rebase":
        sh(["git", "checkout", "-q", case["operation"]["branch"]], repo, check=True)
    p = sh(args + [op, rev], repo, env=env, timeout=timeout)
    try:
        v = json.loads(p.stdout)
    except json.JSONDecodeError:
        v = {"error": p.stdout[-800:] + p.stderr[-800:]}
    attempts = [a.get("by") for s in v.get("steps", []) for a in s.get("attempts", [])]
    return {
        "reported_success": p.returncode == 0 and v.get("state") in ("succeeded", "fast_forward"),
        "state": v.get("state"),
        "attempts": attempts,
        "agent_calls": sum(1 for a in attempts if a != "git"),
        "warnings": v.get("warnings", []),
        "detail": (v.get("error") or "")[-1500:],
    }


def score(case, repo, work):
    """Run visible and hidden tests on a copy of the arm's working tree."""
    d = work / "score"
    if d.exists():
        shutil.rmtree(d)
    shutil.copytree(repo, d, ignore=shutil.ignore_patterns(".git", "__pycache__"))
    vis = sh("python3 -m unittest discover -q", d, timeout=300)
    write_files(d, case["hidden"])
    mods = [p[:-3].replace("/", ".") for p in case["hidden"]]
    hid = sh(["python3", "-m", "unittest", "-q"] + mods, d, timeout=300)
    return {"visible_pass": vis.returncode == 0, "hidden_pass": hid.returncode == 0,
            "hidden_output": (hid.stdout + hid.stderr)[-1200:]}


def reference_agent(case, out):
    """A deterministic 'agent' that writes the known-good solution (offline mode)."""
    path = out / f"reference_{case['id']}.py"
    path.write_text("import sys, os, json\nsys.stdin.read()\nfiles = json.loads(%r)\n"
                    "for rel, c in files.items():\n"
                    "    if c is None:\n        os.path.exists(rel) and os.remove(rel)\n        continue\n"
                    "    os.makedirs(os.path.dirname(rel) or '.', exist_ok=True)\n    open(rel, 'w').write(c)\n"
                    "print('RESULT: SUCCESS')\n" % json.dumps(case["reference"]))
    return f"python3 {path}"


def stale_agent(case, repo):
    """Fault injection: restore the ORIGINAL change's files verbatim (resurrecting
    old paths/names), then claim success. A classic mistake when code diverged."""
    op, rev = case["operation"]["op"], case["operation"]["rev"]
    git = lambda *a: sh(["git", *a], repo).stdout.strip()
    if op == "revert":
        sha = git("rev-parse", rev)
        src, files = f"{sha}^", git("show", "--name-only", "--format=", sha).split()
    elif op == "merge":
        base = git("merge-base", "HEAD", rev)
        src, files = git("rev-parse", rev), git("diff", "--name-only", base, rev).split()
    elif op == "rebase":
        br = case["operation"]["branch"]
        base = git("merge-base", br, rev)
        src, files = git("rev-parse", br), git("diff", "--name-only", base, br).split()
    else:
        sha = git("rev-parse", rev)
        src, files = sha, git("show", "--name-only", "--format=", sha).split()
    keep = [f for f in files if sh(["git", "cat-file", "-e", f"{src}:{f}"], repo).returncode == 0]
    return f"cat > /dev/null; git checkout {src} -- {' '.join(keep)} && git reset -q; echo 'RESULT: SUCCESS'"


def verify_gate(case, repo, gitent, env_extra=None):
    """`gitent verify --working-tree` on an arm's (uncommitted) result: the
    ADR 0011 gate. Returns (verified, seconds, detail). No agent is invoked."""
    op, rev = case["operation"]["op"], case["operation"]["rev"]
    git = lambda *a: sh(["git", *a], repo).stdout.strip()
    if op == "rebase":
        br = case["operation"]["branch"]
        base = git("merge-base", br, rev)
        checks = [(c, "apply") for c in git("rev-list", "--reverse", f"{base}..{br}").split()]
    else:
        checks = [(rev, {"cherry-pick": "apply", "revert": "revert", "merge": "merge"}[op])]
    t0, details = time.time(), []
    env = dict(CLEAN_ENV, **(env_extra or {}))
    ok = True
    for src, kind in checks:
        p = sh([gitent, "--json", "--provider", "none", "verify", src, "--working-tree", "--kind", kind], repo, env=env)
        try:
            v = json.loads(p.stdout)
        except json.JSONDecodeError:
            v = {"error": p.stdout + p.stderr}
        if p.returncode != 0:
            ok = False
            details.append((v.get("error") or "")[-800:])
    return ok, round(time.time() - t0, 1), "\n".join(details)


def run_case(case, args, out):
    res = {"id": case["id"], "kind": case["kind"], "description": case["description"], "arms": {}}
    agent_cmd = reference_agent(case, out) if args.agent == "offline" else AGENTS.get(args.agent, args.agent)
    for arm in [a for a in args.arms if a != "baseline+verify"]:
        work = out / case["id"] / arm
        repo = work / "repo"
        build(case, repo, args.gitent)
        if args.agent == "stale":
            agent_cmd = stale_agent(case, repo)
        t0 = time.time()
        try:
            if arm == "git":
                r = git_arm(case, repo)
            elif arm == "baseline":
                r = baseline_arm(case, repo, agent_cmd, args.timeout)
            else:
                r = gitent_arm(case, repo, args.gitent, agent_cmd, args.llm, args.timeout)
        except subprocess.TimeoutExpired:
            r = {"reported_success": False, "detail": "timeout"}
        r["seconds"] = round(time.time() - t0, 1)
        gate = None
        if arm == "baseline" and "baseline+verify" in args.arms:
            gate = verify_gate(case, repo, args.gitent)
        r.update(score(case, repo, work))
        r["false_success"] = r["reported_success"] and not r["hidden_pass"]
        res["arms"][arm] = r
        if gate is not None:
            g = dict(r)
            g["verified"], g["verify_seconds"], g["detail"] = gate
            g["reported_success"] = r["reported_success"] and gate[0]
            g["seconds"] = round(r["seconds"] + gate[1], 1)
            g["false_success"] = g["reported_success"] and not g["hidden_pass"]
            res["arms"]["baseline+verify"] = g
        print(f"  {case['id']:<32} {arm:<9} hidden={'PASS' if r['hidden_pass'] else 'fail'} "
              f"reported={'success' if r['reported_success'] else 'failure'} ({r['seconds']}s)", flush=True)
    return res


def markdown(results, meta):
    arms = meta["arms"]
    lines = [f"### Run {meta['started']} — agent: `{meta['agent']}`, gitent LLM: `{meta['llm'] or 'none'}`", "",
             "| case | " + " | ".join(arms) + " |", "|---|" + "---|" * len(arms)]
    for r in results:
        cells = []
        for a in arms:
            x = r["arms"][a]
            mark = "✅" if x["hidden_pass"] else "❌"
            rep = "reported success" if x["reported_success"] else "reported failure"
            extra = ""
            if a == "gitent":
                extra = f", agent calls: {x.get('agent_calls', 0)}"
            if x["false_success"]:
                extra += " ⚠️ FALSE SUCCESS"
            cells.append(f"{mark} {rep}{extra}")
        lines.append(f"| {r['id']} | " + " | ".join(cells) + " |")
    lines += ["", "| arm | hidden pass | false successes |", "|---|---|---|"]
    for a in arms:
        hp = sum(r["arms"][a]["hidden_pass"] for r in results)
        fs = sum(r["arms"][a]["false_success"] for r in results)
        lines.append(f"| {a} | {hp}/{len(results)} | {fs} |")
    return "\n".join(lines) + "\n"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--agent", default="offline", help="offline | claude-code | <any shell command>")
    ap.add_argument("--arms", default="git,baseline,gitent")
    ap.add_argument("--llm", default=None, help="LLM command for gitent's verifier/merge analyst (e.g. 'claude -p')")
    ap.add_argument("--filter", default="")
    ap.add_argument("--gitent", default=str(ROOT / "target/release/gitent"))
    ap.add_argument("--timeout", type=int, default=1200)
    ap.add_argument("--jobs", type=int, default=3)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()
    args.arms = args.arms.split(",")
    started = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    out = Path(args.out or (HERE / "runs" / f"{started}-{args.agent if args.agent in AGENTS or args.agent in ('offline', 'stale') else 'custom'}"))
    out.mkdir(parents=True, exist_ok=True)
    cases = [c for c in CASES if args.filter in c["id"]]
    meta = {"started": started, "agent": args.agent, "llm": args.llm, "arms": args.arms,
            "git": sh(["git", "--version"], ROOT).stdout.strip(),
            "agent_version": sh("claude --version", ROOT).stdout.strip() if args.agent.startswith("claude-code") else "",
            "agent_command": AGENTS.get(args.agent, args.agent)}
    print(f"gitent eval: {len(cases)} case(s), arms={args.arms}, agent={args.agent}, out={out}", flush=True)
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        results = list(ex.map(lambda c: run_case(c, args, out), cases))
    (out / "results.json").write_text(json.dumps({"meta": meta, "results": results}, indent=2))
    md = markdown(results, meta)
    (out / "report.md").write_text(md)
    print("\n" + md)


if __name__ == "__main__":
    main()
