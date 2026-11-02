# Evals

Compare plain git, a coding agent on its own, and Gitent driving the same agent.
The scenarios are diverged histories, scored with hidden tests. Results and
interpretation are in [RESULTS.md](RESULTS.md).

```bash
cargo build --release
python3 evals/run.py --agent offline                                  # reference solutions: validates harness + hidden tests (free)
python3 evals/run.py --agent stale --arms baseline,gitent             # fault injection: does anyone report success on wrong work? (free)
python3 evals/run.py --agent claude-code --llm "claude -p"            # live (costs tokens)
python3 evals/run.py --agent "<any coding-agent command>" --filter E1  # bring your own agent; prompt arrives on stdin
```

* Scenarios live in `cases.py`. Each has a history, an operation, hidden tests,
  and a reference solution.
* Output goes to `evals/runs/<timestamp>/` (git-ignored). Copy `report.md` and
  `results.json` into `results/` to keep a run.
