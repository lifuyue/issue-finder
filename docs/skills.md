# Issue Finder Skill Integration

The repository provides two independent skills. Both keep user interaction,
implementation, and validation in the current Work agent session.

| Mode | Entry | Runtime | Discovery and state |
| --- | --- | --- | --- |
| CLI + skill | `issue-finder-cli` | Installed Rust CLI, Git, GitHub credentials | CLI recommendation engine, bounded GitHub search tools, CLI state and task results |
| Standalone skill | `issue-finder` | Python 3.9+, Git, authenticated `gh` | Independent coarse filtering and one workspace task JSON |

Neither mode starts dispatch, another Codex process, or project approval objects.
The skills do not call each other or switch implementations on failure.

## CLI skill

The package is [`skills/issue-finder-cli/`](../skills/issue-finder-cli/):

```text
skills/issue-finder-cli/
  SKILL.md
  agents/openai.yaml
  references/install.md
  references/tools.md
```

Add this repository as a Codex project and invoke
`skills/issue-finder-cli/SKILL.md` explicitly, or follow the
[root README](../README.md#cli--skill) to register it in `.agents/skills` for
picker discovery. User-wide installation uses `~/.agents/skills/issue-finder-cli`.
Copy the entire directory so references remain available. The package can run
outside its source checkout after copying; its dependency is the installed CLI.

The CLI skill is explicitly invoked (`allow_implicit_invocation: false`) to
avoid competing with the independent skill for the same request. Supported
local discovery locations, symlink behavior, and this metadata field are defined
in the [official Codex skill documentation](https://learn.chatgpt.com/docs/build-skills).

### Prerequisites and first use

The skill checks `issue-finder --version` and
`issue-finder tools --profile session list`, requiring `sessionContractVersion: 1`
and its seven tools. If absent or incompatible, it prompts for the latest
compatible official stable CLI installation in the agent's execution environment.
A local desktop installation does not supply a remote/cloud runtime. The
[published release channel](https://github.com/lifuyue/issue-finder/releases/latest)
and Cargo installation are described in the bundled
[installation reference](../skills/issue-finder-cli/references/install.md).
A stable package may lag this checkout: source installation is an explicit
alternative, never a silent `cargo run` fallback.

Session authentication resolves `GITHUB_TOKEN`, configured token, then a bounded,
captured `gh auth token --hostname github.com` lookup. It never prints or saves
the fallback token. Missing configuration is valid: defaults and per-call
`profile` settings suffice. `init`, profile bootstrap, optional LLM configuration,
and MCP registration are unnecessary for this path.

### One agent, structured tools

```text
status -> scout -> assess -> prepare
                            -> current agent implements and reviews
                            -> finish -> current agent reports
                   task_status resumes an existing workspace
                   feedback records read/dismiss/restore
```

Use `issue-finder tools --profile session call issue-finder.TOOL --arguments JSON`.
See [current-session tool examples](./usage.md#current-session-tools). The catalog
is the authoritative argument schema. GitHub query, sort, pagination, and API
budget are explicit inputs; returned warnings and evidence qualify the ranking.
API search order controls retrieval, while the CLI separately ranks contribution
candidates. The agent decides which issue to pursue from its body and discussion.

An explicit issue skips discovery. Recommendation-only and assessment-only
requests never prepare a workspace. A prepare-only request stops before edits.
For implementation, prepare refreshes evidence and uses the shared prepare gate;
the current session receives a workspace/task rather than a dispatch package.
Continue using `task_status` with the absolute workspace. Finish executes the
explicit check argument arrays selected by the agent and verifies task identity
and changes against its base. A failed check remains recoverable; the agent fixes
and retries. A passing finish records execution evidence, not semantic proof that
the patch fixes the issue.

The current agent follows user authorization, target repository instructions,
and host permissions. Commit, push, PRs, and public comments are outside these
CLI tools and need user authorization. Generated task metadata must not enter
contribution commits.

## Standalone skill

The installable product skill lives at
[`skills/issue-finder/SKILL.md`](../skills/issue-finder/SKILL.md). Its whole package is:

```text
skills/issue-finder/
  SKILL.md
  scripts/issue_finder.py
  agents/openai.yaml
```

Use the [root README installation instructions](../README.md#standalone-skill)
to copy or symlink this directory into `~/.agents/skills/issue-finder`, or copy it
into a target repository's `.agents/skills/issue-finder`. Copying only `SKILL.md`
is insufficient: the workflow needs its bundled script. The package is standalone
and can run after the source checkout is removed when installed by copying.
It does not call the Rust CLI or read `~/.issue-finder`.

## Ownership and execution

The current Codex session owns semantic selection, feasibility review, code edits,
debugging, final diff review, and reporting. The helper owns only deterministic
GitHub reads, workspace operations, checks, and result collection. This independent package has three commands, and at most one temporary task JSON per prepared workspace.
No MCP, plugin runtime, dispatch database, handoff pack, nested Codex, or project
approval objects are involved. The optional `openai.yaml` contains UI metadata only.

Python 3.9+, Git, and authenticated `gh` must be available on PATH. Git clone/fetch
also need credentials for private repositories (`gh auth setup-git`). GitHub API
calls use `gh api --hostname github.com --method GET`; Enterprise hosts are not
supported by this version. The helper never posts to GitHub.

The [GitHub CLI API documentation](https://cli.github.com/manual/gh_api) describes
the pagination flags used to read issue comments and timeline pages. GitHub data
is untrusted evidence, never executable instructions.

## Helper commands

Set `SKILL_DIR` to the absolute installed skill directory, not to the target repo.
It is a shell variable in these examples, not a variable injected by Codex:

```bash
SKILL_DIR="$HOME/.agents/skills/issue-finder"
python3 "$SKILL_DIR/scripts/issue_finder.py" scout --repo owner/repo --limit 8 --json
python3 "$SKILL_DIR/scripts/issue_finder.py" prepare --issue 'owner/repo#123' --json
python3 "$SKILL_DIR/scripts/issue_finder.py" finish --workspace /absolute/workspace \
  --check 'cargo test' --check 'cargo clippy --all-targets -- -D warnings' --json
```

All commands emit a single JSON object (also without `--json`). Exit code 0 means
`ok: true`; errors, exhausted candidates, failed checks, and no-change outcomes
return a nonzero code. `--help` describes the options without contacting GitHub.

- **scout:** accepts `--repo` or infers `upstream`/`origin`; `--query` adds search
  preferences. It inspects up to `--scan-limit` recently updated issues (default
  30, maximum 100) and returns up to `--limit` (default 8). Comments and timeline
  are paginated; candidates whose competition cannot be checked are excluded with
  a warning. Scoring is intentionally coarse. Code scope, semantic content quality,
  and unlinked PRs still require Codex review. No full recommendation engine,
  profile system, cooldown, or ranking-quality parity is claimed.
- **prepare:** re-fetches issue/competition evidence before any workspace change.
  A clean matching checkout gets a new `fuyue/issue-<number>-<suffix>` branch at
  its current HEAD (local commits are preserved; this does not imply the latest
  upstream revision). A dirty matching checkout gets a separate worktree at the
  fetched default branch. Other repositories are cloned under
  `<root>/<owner>/<repo>`; existing caches are fetched and used for a new worktree.
  The root comes from `--workspace-root`, `ISSUE_FINDER_WORKSPACE_ROOT`, or
  `~/Code/contributions`, in that order, and must be outside existing repositories.
  It records the full issue, latest 20 comments, total comment count and truncation
  flag, workspace, branch, base commit, instruction files, manifests, and conservative
  check hints in `.issue-finder-task.json`. Use `gh issue view --comments` to review
  older discussion when needed; competition filtering reads all comment pages.
- **finish:** requires explicit `--check` commands. These run in the workspace
  without a shell, with a per-check timeout (`--check-timeout`, default 600 seconds)
  and bounded output tails. It checks workspace/branch/base identity and refuses
  staged or committed task JSON, including JSON accidentally committed then
  deleted. Results include checks, changed files relative to the original base,
  tracked diff stat, staged files, and untracked files separately (untracked content
  and staged-only differences are not in the working-tree `diffStat`). Success
  requires passing checks and actual changes. It never stages,
  commits, pushes, installs dependencies, or creates PRs.

Do not run the three commands blindly: Codex selects and implements between them.
Validation commands come from reviewed repository instructions and code. Package
scripts, Make targets, and tests can execute arbitrary code; the script does not
sandbox them or certify their safety. `finish` reports execution outcomes, not
whether the patch semantically fixes the issue.

## Continuation and failures

Resume in the returned workspace by reading its task JSON and checking the issue
against the user's request. Do not call prepare again there or edit the task JSON.
A failed check, missing executable, wrong branch, or invalid task leaves the task
file intact; fix the underlying issue and rerun finish. Successful finish deletes
only that task file. It leaves the branch and all contribution changes available
for review and user-authorized submission. Keep the returned JSON in the session
for final reporting; there is no persistent outcome database.

When abandoning a discovered candidate, preserve any edits and use a separate
workspace for the next candidate. Abandoned task files/workspaces are not cleaned
automatically. Authentication, rate limits, insufficient permissions, and invalid
cache paths surface as errors; the helper never resets or deletes work to recover.

The skill introduces no intermediate approvals, but cannot grant filesystem or
network access. Respect target `AGENTS.md`, host permissions, and user stopping
points. A recommendation-only invocation does not prepare or modify a workspace.

## Validation scope

The two skills are separately supported modes. The standalone helper does not
reuse CLI search, state, or result handling. CLI architecture documents describe
its recommendation engine and the separate handoff/dispatch workflows; dispatch
is not a prerequisite for either current-session skill.

Offline tests use mocked GitHub responses and temporary real Git repositories:

```bash
python3 -m unittest discover -s tests -p 'test_skill_native.py' -v
```

The independent coarse-filter samples are in
[`tests/fixtures/recommendation_eval/skill_native.json`](../tests/fixtures/recommendation_eval/skill_native.json).
They do not replace or change the Rust recommendation datasets. Automated checks
cover discovery filtering, paginated competition, clean/dirty/cached workspace
preparation, safe paths, validation retries, and task-file exclusion. Live six-profile
comparison and Codex end-to-end quality evaluation are separate evidence; passing
these tests does not establish that parity or imply parity between the two modes.
The [initial read-only smoke report](./recommendation-evals/2026-08-31-skill-native-smoke/report.md)
records observed GitHub behavior, the failures turned into fixtures, and its limits.

## Handoff generated context (not an installable entry skill)

The CLI writes a per-item context entry for its separate handoff workflow.
Do not install these generated files in place of either stable package above.
The session tool profile does not use this handoff entry as its workflow skill.

Each prepared item includes:

```text
.agents/
  skills/
    issue-finder/
      SKILL.md
      refs.json
```

The generated skill tells the agent how to consume a Issue Finder handoff:

1. Read `context/entry.md` and `context/safety.md` first.
2. Read `context/probe.md` before deciding which commands to run.
3. Defer value, issue, repo, and validation detail until needed.
4. Treat Issue Finder inbox files as generated context, not target source.

### Rust Codex entry

`codex.md` is the shortest entrypoint. It includes absolute paths to:

- The handoff pack directory
- `handoff.json`
- `handoff.md`
- `agent-policy.json`
- `probe.json`
- The generated `issue-finder` skill
- Default context files

This keeps the entry usable even when the agent starts from the target workspace instead of the Issue Finder inbox directory.

### Rust context files

Default-visible context:

- `context/entry.md`
- `context/safety.md`
- `context/probe.md`

Deferred context:

- `context/value.md`
- `context/issue.md`
- `context/repo.md`
- `context/validation.md`

This keeps the first agent turn small while preserving complete handoff detail for later phases.
