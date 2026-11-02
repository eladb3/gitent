#!/usr/bin/env python3
"""Real-backport benchmark (criterion F4).

Mines real backports from a project that records them in commit messages
("Backport of <sha> from main", as Django does). Each case pairs:

  X       the original fix on main (what every arm is asked to move)
  target  the parent of the human backport on the stable branch
  B       the human backport, whose test changes are the HIDDEN oracle

A case is kept only if B's tests fail on `target` (with B's test files overlaid)
and pass on B, so the hidden tests discriminate the change.

The arms are the same as in evals/run.py: git, baseline, and gitent. No arm can
see B: each arm works in its own clone that holds only two refs, `main` (at
target) and `source` (at X).

  python3 evals/real_backports.py mine --mirror /path/django.git --python /path/venv/bin/python
  python3 evals/real_backports.py run  --cases evals/real/django_cases.json --agent claude-code --llm "claude -p"
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
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import run as base  # noqa: E402  (reuses sh, agent presets, report format)

BACKPORT_RE = re.compile(r"Backport of ([0-9a-f]{7,40}) from main", re.I)


def git(repo, *a, check=True):
    p = base.sh(["git", *a], repo)
    if check and p.returncode != 0:
        raise RuntimeError(f"git {a} failed: {p.stderr}")
    return p.stdout


def changed(repo, rev):
    out = git(repo, "show", "--name-status", "--format=", rev)
    files = []
    for line in out.splitlines():
        parts = line.split("\t")
        if parts and parts[0]:
            files.append((parts[0][0], parts[-1]))
    return files


def is_test_path(p):
    return p.startswith("tests/")


def labels(paths):
    """Django test labels for test modules under tests/."""
    out = []
    for p in paths:
        if not (p.startswith("tests/") and p.endswith(".py")):
            continue
        name = p.rsplit("/", 1)[-1]
        if not name.startswith("test"):
            continue
        out.append(p[len("tests/"):-3].replace("/", "."))
    return sorted(set(out))


def runtests_cmd(py, lab):
    return f"{py} tests/runtests.py --parallel=1 --noinput {' '.join(lab)}"


def worktree(mirror, rev):
    d = Path(tempfile.mkdtemp(prefix="rb-"))
    git(mirror, "worktree", "add", "--detach", "-f", str(d / "wt"), rev)
    return d / "wt"


def drop_worktree(mirror, path):
    base.sh(["git", "worktree", "remove", "--force", str(path)], mirror)
    shutil.rmtree(path.parent, ignore_errors=True)


def overlay_tests(mirror, b, files, dest):
    for status, p in files:
        if not is_test_path(p):
            continue
        target = Path(dest) / p
        if status == "D":
            if target.exists():
                target.unlink()
            continue
        blob = subprocess.run(["git", "show", f"{b}:{p}"], cwd=mirror, capture_output=True).stdout
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(blob)


def run_hidden(py, wt, lab, timeout=900):
    p = base.sh(runtests_cmd(py, lab), wt, timeout=timeout)
    return p.returncode == 0, (p.stdout + p.stderr)[-1500:]


# ------------------------------------------------------------------ mining

def mine(args):
    mirror = Path(args.mirror)
    cands = []
    for br in args.branches.split(","):
        fork = git(mirror, "merge-base", "main", br).strip()
        log = git(mirror, "log", "--no-merges", "--format=%H%x00%B%x01", f"{fork}..{br}")
        for entry in log.split("\x01"):
            if "\x00" not in entry:
                continue
            b, msg = entry.strip().split("\x00", 1)
            m = BACKPORT_RE.search(msg)
            if not m:
                continue
            x = base.sh(["git", "rev-parse", "--verify", "-q", m.group(1) + "^{commit}"], mirror).stdout.strip()
            if not x:
                continue
            cands.append({"branch": br, "B": b, "X": x, "subject": msg.splitlines()[0]})
    print(f"{len(cands)} backport candidates", flush=True)

    kept = []
    stats = {"identical": 0, "clean_differs": 0, "conflict": 0, "rejected": 0}
    for c in cands:
        if len(kept) >= args.max:
            break
        fx, fb = changed(mirror, c["X"]), changed(mirror, c["B"])
        code_x = [p for _, p in fx if p.startswith("django/") and p.endswith(".py")]
        lab_b = labels([p for s, p in fb if s != "D"])
        numstat = git(mirror, "show", "--numstat", "--format=", c["X"])
        size = sum(int(a) + int(d) for a, d, *_ in (l.split("\t") for l in numstat.splitlines()) if a.isdigit() and d.isdigit())
        if not code_x or not lab_b or size > args.max_lines or len(git(mirror, "rev-list", "--parents", "-n1", c["B"]).split()) != 2:
            stats["rejected"] += 1
            continue
        target = git(mirror, "rev-parse", c["B"] + "^").strip()
        mt = base.sh(["git", "merge-tree", "--write-tree", f"--merge-base={c['X']}^", target, c["X"]], mirror)
        b_tree = git(mirror, "rev-parse", c["B"] + "^{tree}").strip()
        if mt.returncode != 0:
            kind = "conflict"
        else:
            kind = "identical" if mt.stdout.split()[0] == b_tree else "clean_differs"
        if kind == "identical" and stats["identical"] >= args.max_identical:
            continue
        # Validity: hidden tests must fail on target (B's tests overlaid) and pass on B.
        wt = worktree(mirror, target)
        try:
            overlay_tests(mirror, c["B"], fb, wt)
            fail_before, _ = run_hidden(args.python, wt, lab_b)
        finally:
            drop_worktree(mirror, wt)
        if fail_before:
            stats["rejected"] += 1
            print(f"  skip {c['B'][:10]} (hidden tests pass without the change)", flush=True)
            continue
        wt = worktree(mirror, c["B"])
        try:
            pass_after, out = run_hidden(args.python, wt, lab_b)
        finally:
            drop_worktree(mirror, wt)
        if not pass_after:
            stats["rejected"] += 1
            print(f"  skip {c['B'][:10]} (hidden tests fail on the human backport: env issue?)", flush=True)
            continue
        lab_x_target = [l for l in labels([p for _, p in fx]) if base.sh(["git", "cat-file", "-e", f"{target}:tests/{l.replace('.', '/')}.py"], mirror).returncode == 0]
        if not lab_x_target:
            lab_x_target = sorted({l.split(".")[0] for l in labels([p for _, p in fx])}) or sorted({l.split(".")[0] for l in lab_b})
        c.update(kind=kind, target=target, size=size, hidden_labels=lab_b, visible_labels=lab_x_target,
                 hidden_files=[[s, p] for s, p in fb if is_test_path(p)])
        stats[kind] += 1
        kept.append(c)
        print(f"  keep {c['B'][:10]} {kind:<13} {size:>4} lines  {c['subject'][:70]}", flush=True)
        save(args, mirror, stats, kept)
    save(args, mirror, stats, kept)
    print(json.dumps(stats))


def save(args, mirror, stats, kept):
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"mirror": str(mirror), "stats": stats, "cases": kept}, indent=2))


# ------------------------------------------------------------------ running

def make_repo(mirror, case, dest, gitent, py):
    base.sh(["git", "clone", "-q", "--shared", "--no-tags", "--no-checkout", str(mirror), str(dest)], dest.parent, timeout=600)
    git(dest, "checkout", "-q", "-B", "main", case["target"])
    git(dest, "branch", "-f", "source", case["X"])
    base.sh(["git", "remote", "remove", "origin"], dest)
    for ref in git(dest, "for-each-ref", "--format=%(refname)").split():
        if ref not in ("refs/heads/main", "refs/heads/source"):
            git(dest, "update-ref", "-d", ref)
    git(dest, "config", "user.email", "eval@example.com")
    git(dest, "config", "user.name", "Eval")
    cmd = runtests_cmd("python", case["visible_labels"])  # venv python is first on PATH
    cfg = dest / ".git/gitent/config.toml"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text(f'[validation]\ntest_command = {json.dumps(cmd)}\ntest_selector = {json.dumps(cmd + " -k {test}")}\ntimeout_secs = 1200\n')


def baseline_prompt(case, repo, py):
    msg = git(repo, "log", "-1", "--format=%B", "source")
    diff = git(repo, "show", "--format=", "source")
    return (f"You are working in a git checkout of Django (the current directory), on branch main, which is an older "
            f"release branch.\n\nTask: backport the change from commit {case['X'][:10]} (on the development branch) "
            f"to this branch. The code here differs from where the commit was written; adapt as needed and keep "
            f"everything else working.\n\nCommit message:\n{msg}\nCommit diff:\n```diff\n{diff}```\n\n"
            f"Run tests with: {runtests_cmd('python', case['visible_labels'])}\n"
            f"Edit files directly. Do not commit and do not change git history.\n"
            f"When you are done, print a final line `RESULT: SUCCESS` if the task is fully done, otherwise `RESULT: FAILURE`.\n")


def score(case, repo, mirror, py, work):
    d = work / "score"
    if d.exists():
        shutil.rmtree(d)
    shutil.copytree(repo, d, ignore=shutil.ignore_patterns(".git", "__pycache__"), symlinks=True)
    overlay_tests(mirror, case["B"], [tuple(x) for x in case["hidden_files"]], d)
    ok, out = run_hidden(py, d, case["hidden_labels"])
    shutil.rmtree(d, ignore_errors=True)
    return {"hidden_pass": ok, "hidden_output": out}


def run_case(case, args):
    cid = f"{case['branch'].replace('/', '-')}-{case['B'][:10]}"
    res = {"id": cid, "kind": case["kind"], "description": case["subject"], "arms": {}}
    env_path = str(Path(args.python).parent) + os.pathsep + os.environ.get("PATH", "")
    base.CLEAN_ENV["PATH"] = env_path
    for arm in args.arms:
        work = Path(args.out) / cid / arm
        repo = work / "repo"
        if work.exists():
            shutil.rmtree(work)
        work.mkdir(parents=True)
        make_repo(Path(args.mirror), case, repo, args.gitent, args.python)
        agent = base.AGENTS.get(args.agent, args.agent)
        t0 = time.time()
        try:
            if arm == "git":
                p = base.sh(["git", "cherry-pick", "source"], repo)
                ok = p.returncode == 0
                if not ok:
                    base.sh(["git", "cherry-pick", "--abort"], repo)
                r = {"reported_success": ok, "detail": (p.stdout + p.stderr)[-500:]}
            elif arm == "baseline":
                p = base.sh(agent, repo, stdin=baseline_prompt(case, repo, args.python), timeout=args.timeout)
                out = p.stdout + p.stderr
                m = re.findall(r"RESULT:\s*(SUCCESS|FAILURE)", out)
                r = {"reported_success": bool(m) and m[-1] == "SUCCESS", "detail": out[-1500:]}
            else:
                fake = {"operation": {"op": "cherry-pick", "rev": "source"}}
                r = base.gitent_arm(fake, repo, args.gitent, agent, args.llm, args.timeout)
        except subprocess.TimeoutExpired:
            r = {"reported_success": False, "detail": "timeout"}
        r["seconds"] = round(time.time() - t0, 1)
        r.update(score(case, repo, Path(args.mirror), args.python, work))
        r["false_success"] = r["reported_success"] and not r["hidden_pass"]
        res["arms"][arm] = r
        print(f"  {cid:<28} {case['kind']:<13} {arm:<9} hidden={'PASS' if r['hidden_pass'] else 'fail'} "
              f"reported={'success' if r['reported_success'] else 'failure'} ({r['seconds']}s)", flush=True)
        shutil.rmtree(repo, ignore_errors=True)
    return res


def run(args):
    data = json.loads(Path(args.cases).read_text())
    args.mirror = args.mirror or data["mirror"]
    cases = [c for c in data["cases"] if args.filter in c["B"] or args.filter in c["kind"]][: args.limit]
    args.arms = args.arms.split(",")
    started = dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H-%M-%SZ")
    args.out = args.out or str(HERE / "runs" / f"{started}-real-{args.agent}")
    Path(args.out).mkdir(parents=True, exist_ok=True)
    print(f"real-backport eval: {len(cases)} case(s), arms={args.arms}, agent={args.agent}", flush=True)
    meta = {"started": started, "agent": args.agent, "llm": args.llm, "arms": args.arms, "source": "django real backports",
            "agent_command": base.AGENTS.get(args.agent, args.agent)}
    results = []
    with cf.ThreadPoolExecutor(max_workers=args.jobs) as ex:
        futs = [ex.submit(run_case, c, args) for c in cases]
        for f in cf.as_completed(futs):
            try:
                results.append(f.result())
            except Exception as e:  # keep going; record the harness error
                print(f"  harness error: {e}", flush=True)
            (Path(args.out) / "results.json").write_text(json.dumps({"meta": meta, "results": results}, indent=2))
    results.sort(key=lambda r: r["id"])
    md = base.markdown(results, meta)
    by_kind = {}
    for r in results:
        for a in args.arms:
            k = by_kind.setdefault((r["kind"], a), [0, 0])
            k[0] += r["arms"][a]["hidden_pass"]
            k[1] += 1
    md += "\n| kind | " + " | ".join(args.arms) + " |\n|---|" + "---|" * len(args.arms) + "\n"
    for kind in ("identical", "clean_differs", "conflict"):
        if any(k == kind for k, _ in by_kind):
            md += f"| {kind} | " + " | ".join(f"{by_kind[(kind, a)][0]}/{by_kind[(kind, a)][1]}" for a in args.arms) + " |\n"
    (Path(args.out) / "report.md").write_text(md)
    print("\n" + md)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    m = sub.add_parser("mine")
    m.add_argument("--mirror", required=True)
    m.add_argument("--python", required=True)
    m.add_argument("--branches", default="stable/6.1.x,stable/6.0.x,stable/5.2.x")
    m.add_argument("--max", type=int, default=24)
    m.add_argument("--max-identical", type=int, default=4)
    m.add_argument("--max-lines", type=int, default=300)
    m.add_argument("--out", default=str(HERE / "real/django_cases.json"))
    r = sub.add_parser("run")
    r.add_argument("--cases", default=str(HERE / "real/django_cases.json"))
    r.add_argument("--mirror", default=None)
    r.add_argument("--python", required=True)
    r.add_argument("--agent", default="claude-code")
    r.add_argument("--llm", default=None)
    r.add_argument("--arms", default="git,baseline,gitent")
    r.add_argument("--gitent", default=str(ROOT / "target/release/gitent"))
    r.add_argument("--timeout", type=int, default=1800)
    r.add_argument("--jobs", type=int, default=3)
    r.add_argument("--filter", default="")
    r.add_argument("--limit", type=int, default=1000)
    r.add_argument("--out", default=None)
    a = ap.parse_args()
    mine(a) if a.cmd == "mine" else run(a)


if __name__ == "__main__":
    main()
