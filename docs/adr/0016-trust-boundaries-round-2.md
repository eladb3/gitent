# 0016: Trust boundaries, round 2

**Status:** accepted. **Extends:** 0014 (and supersedes its §1 on where a
generated `serve` token goes). **Prompted by:** the second external security
review (2026-11-07).

## Context

ADR 0014 stopped other parties from *choosing* the commands Gitent runs. The
second review found five ways that was not enough:

| finding | how |
|---|---|
| Children inherit Gitent's environment | PR tests in CI saw `GH_TOKEN`, `ANTHROPIC_API_KEY`, `ACTIONS_*` and could write `BASH_ENV` into `$GITHUB_ENV`, which runs PR code in a later step that holds `GH_TOKEN`. |
| The Action's source build ran in the PR checkout | cargo reads `.cargo/config.toml` (and rustup `rust-toolchain.toml`) from the current directory, so the PR could choose the compiler wrapper of the binary that verifies it. |
| An agent could still run commands | `gitent_commit` over MCP stores a `check.run` (origin `agent`), and `gitent_check` ran it. An agent that edits files could also write `.gitent/config.toml`, which every MCP/HTTP call re-reads. |
| Cloning was enough | the shared `.gitent/config.toml` could set `executor.command`, `llm.command` or `llm.base_url`, which run (or receive API keys) on the user's next operation. |
| `serve` | a non-loopback bind was allowed with a warning, and a generated token was printed to stderr (logs, scrollback). |

## Decision

1. **Allowlisted child environments.** `proc::run_shell_with` takes an
   `EnvPolicy`; every child starts from an empty environment.
   - `RepoCode` (test commands, test selectors, requirement `check.run`, the
     builtin agent's `run` tool): `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`,
     `LANG`/`LC_*`, `TERM`, `TZ`, `TMPDIR`, `CI`, git identity
     (`GIT_AUTHOR_*`, `GIT_COMMITTER_*`), CA bundle paths, `XDG_*`, and
     toolchain variables (`CARGO_*`, `RUSTUP_*`, `GOPATH`/`GOCACHE`/`GOFLAGS`/…,
     `PYTHON*`, `VIRTUAL_ENV`, `NODE_*`, `npm_config_*`, `JAVA_HOME`, …).
     Proxy variables pass only without `user:pass@`.
   - `LlmCli` (`llm.command` and `executor.command`, i.e. the claude-code /
     codex / … presets): the same plus provider variables (`ANTHROPIC_*`,
     `OPENAI_*`, `CLAUDE_*`, `CODEX_*`, `GEMINI_*`).
   - Names that look like secrets (`*TOKEN*`, `*SECRET*`, `*PASSWORD*`,
     `*PASSWD*`, `*CREDENTIAL*`, `*_AUTH*`, `*_KEY`, `*APIKEY`), `ACTIONS_*`
     and the workflow command files (`GITHUB_ENV`, `GITHUB_PATH`,
     `GITHUB_OUTPUT`, `GITHUB_STATE`, `GITHUB_STEP_SUMMARY`) are never passed
     by policy, even when an allowlisted prefix matches. LLM CLIs never get
     `GH_*`, `GITHUB_*` tokens, `ACTIONS_*` or `GITENT_TOKEN`.
   - `GITENT_PASS_ENV` (comma list; `PREFIX_*` allowed) passes named variables
     explicitly, secrets included. It is read from Gitent's environment only,
     never from config.
   - Gitent's own `GITENT_*` role variables are always set.
   - `run_shell` keeps its signature and means `RepoCode`, so the safe policy
     is the default; only the two CLI call sites opt into `LlmCli`.
2. **The Action builds only its own source, isolated.** The fallback builds
   `$GITHUB_ACTION_PATH`, from a fresh directory outside the workspace, with a
   fresh `CARGO_HOME`, `env -i` plus network settings, the toolchain's own
   `cargo`/`rustc` (resolved with `rustup which`, so no proxy consults a
   toolchain file), the toolchain `stable` or the action's own
   `rust-toolchain`, and `--locked`. The verify step gets no GitHub token
   (`GH_TOKEN`/`GITHUB_TOKEN` blanked in `action.yml` and unset in `run.sh`);
   only the comment step does. Every example uses `persist-credentials: false`,
   and `run.sh` warns when it finds a persisted credential. The helper scripts
   run Python with `-I` (found while testing this ADR: `python3 -c 'import
   json'` imported a `json.py` from the PR checkout in steps holding the API
   key and `GH_TOKEN`). The opt-in LLM verifier's key is used by Gitent for
   the provider's HTTPS API; the PR's processes never see it.
3. **Agents cannot make Gitent run commands.**
   - `Runner` skips `command` requirements whose origin is `agent` or `llm`,
     with a reason naming the remedy, unless `requirements.trust_agent_commands
     = true` is in the **local** config (`.git/gitent/config.toml`) or the CLI
     passes `--trust-agent-commands`. The shared config cannot set it; MCP/HTTP
     arguments cannot pass it (nor `trust_notes`): both are in
     `CLI_ONLY_ARGS`. This also skips the commit-time proof of such checks.
   - MCP/HTTP sessions (`Session::open_for_agent`) take the command keys
     (`validation.test_command`, `validation.test_selector`,
     `executor.command`, `llm.command`, `llm.base_url`) from the committed
     HEAD's `.gitent/config.toml` (plus local config and env); other keys
     still come from the working tree.
4. **Direnv-style trust for the shared config.** If `.gitent/config.toml` sets
   `executor.command`, `llm.command` or `llm.base_url`, those keys are ignored
   until the user runs `gitent trust`, which stores the sha256 of the file's
   content in `.git/gitent/trusted`. Any change to the content needs trust
   again. Ignored keys produce one warning line on stderr; read-only commands
   keep working (an `llm.provider = "command"` left without its command
   becomes `none`). `gitent init` trusts the config it writes. The same gate
   applies to the shared config read from HEAD (agents) or a PR base
   (`verify-pr`); CI sets these through the environment instead.
   `validation.test_command` is **not** gated: running a repository's tests
   runs its code whatever the command is, so gating it would add prompts
   without adding safety.
5. **Markdown.** `action/md.py` already escapes everything the Action itself
   writes; `comment.sh` posts Gitent's report and an escaped PR title, and
   passes no raw LLM text. Escaping inside the Rust renderer is out of this
   ADR's scope (ADR 0015).
6. **`serve` and unsandboxed executors.**
   - A non-loopback `--addr` is refused unless `--allow-remote`, which also
     requires an explicit `GITENT_TOKEN` and then accepts any `Host` header
     (token and `Origin` checks remain).
   - A generated token is written to `.git/gitent/serve-token` (mode 0600,
     replaced without following links); stderr names the file, never the
     token. This supersedes ADR 0014 §1's "printed once on stderr".
   - The `aider` preset (`--yes-always`) and the `builtin` executor (its `run`
     tool executes shell commands the LLM chooses) print the unsandboxed
     warning, like `gemini` and `kiro`.

## Consequences

* Test suites that relied on an arbitrary inherited variable must name it in
  `GITENT_PASS_ENV`. Agent CLIs that authenticate through other variables
  (e.g. `AWS_*` for Bedrock, `GOOGLE_*` for Vertex) need it too.
* Agents' command requirements are stored but report `skipped` until a human
  consents; their commit-time proof is recorded as `skipped`/`skipped`
  (the commit is not rejected for it).
* A freshly cloned repository whose config names an agent or LLM CLI needs
  `gitent trust` once (and after every change to the file).
* An agent's working-tree edits to `.gitent/config.toml` take effect for
  agent calls only once committed (and, for gated keys, trusted).

## Known limits

* **An agent with write access to `.git/`** can edit
  `.git/gitent/config.toml` or `.git/gitent/trusted`, as it could edit
  `~/.bashrc`. These defences assume the agent edits the project, not the
  user's account; sandbox the agent (most coding agents can deny `.git/`).
* **An agent that can commit** can put a test command into HEAD. It can also
  edit the tests themselves, which Gitent runs; what it cannot do is make
  Gitent run a program the user never saw (gated keys need `gitent trust`).
* **Earlier workflow steps** that run PR code (`pip install -e .`, `npm ci`)
  can change the runner before the action installs; the release binary
  (checked against its `.sha256`) is the safer default.
* **The allowlist is heuristic.** A secret under an innocuous name that
  matches an allowlisted prefix (say `NODE_REGISTRY_URL` with credentials in
  it) still passes; proxy URLs are the one value that is inspected.
