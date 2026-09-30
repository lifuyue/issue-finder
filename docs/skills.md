# Issue Finder Skill Integration

The CLI skill serves Codex through two business tools: `scout` and `assess`.
Codex owns selection, workspace preparation, reproduction, repair, validation,
review, and authorized PR delivery. The independent Python skill is a separate
legacy package outside this Codex CLI contract; its commands are unchanged.

| Package | Runtime | Role |
| --- | --- | --- |
| `issue-finder-cli` | Installed Rust CLI and GitHub read credentials | Codex discovery, ranking, and assessment evidence |
| Legacy `issue-finder` | Python 3.9+, Git, authenticated `gh` | Independent helper with its own workspace task JSON |

The packages do not call each other or switch implementations on failure.

## CLI skill

The package is [`skills/issue-finder-cli/`](../skills/issue-finder-cli/):

```text
skills/issue-finder-cli/
  SKILL.md
  agents/openai.yaml
  references/install.md
  references/tools.md
```

Invoke `skills/issue-finder-cli/SKILL.md` explicitly, or follow the
[root README](../README.md#cli--skill) to register the entire package in
`.agents/skills/issue-finder-cli` or `~/.agents/skills/issue-finder-cli`.
References must travel with the main file. The skill uses
`allow_implicit_invocation: false` to avoid competing with the independent
package. See the [official Codex skill documentation](https://learn.chatgpt.com/docs/build-skills)
for discovery locations and metadata.

Cloud environment configuration owns CLI installation, dependencies, PATH, and
supported credentials. A local installation does not supply a cloud runtime.
See the [installation reference](../skills/issue-finder-cli/references/install.md).
Negotiate the installed schema with:

```bash
issue-finder tools list
```

The default tool and MCP profiles are `session`. Require
`sessionContractVersion: 2` and exactly `issue-finder.scout` and
`issue-finder.assess`; `--profile session` is the explicit equivalent.
The separate legacy control catalog requires `--profile control` and is outside
this skill. If a stable release lacks version 2, an authorized source installation
from this checkout is an explicit alternative.

Missing configuration is valid. Credentials resolve from `GITHUB_TOKEN`, optional
configured token, then a captured host `gh` authentication lookup. Actual
configuration, authentication, and network errors return directly from the
business call. No separate readiness step, interactive initialization, profile
bootstrap, model API key, or MCP registration is required.

### Calling and interpreting the tools

```text
scout -> assess -> Codex chooses and performs the authorized contribution
         assess also accepts an explicit issue without discovery
```

Use `issue-finder tools call issue-finder.TOOL --arguments JSON`.
See [examples and parameters](./usage.md#current-session-tools) and the
[bundled tool reference](../skills/issue-finder-cli/references/tools.md).
`scout` performs bounded discovery, filtering, and ranking. `assess` collects issue,
discussion, competition, and repository evidence. Carry scout's resolved `profile`
into assessments; overrides are per-call and omission restores configured defaults.
GitHub API sorting controls retrieval, not final recommendation quality.

Scores and recommendation reasons, including low popularity, inform the choice;
they do not establish a repair authorization gate. Read relevant comments and
current code. Partial or truncated evidence and a clear competition score do not
prove there is no linked or unlinked competing PR. Refresh stale evidence when
resuming or before acting on changed ownership/competition; fetch additional
comment pages only as needed. Respect shared rate limits.

The CLI automatically records shown/read events for ranking unless a call opts
out. Historical dismissed/done/prepared states are ignored by Codex rankings;
there is no manual cross-chat ignore/restore entry. The tools do not persist
Codex workspace tasks, run validation commands, or record PR outcomes. Local
checks, a pushed branch, PR creation, CI, and merge are separate facts reported
by Codex. GitHub read access does not prove publication permissions.

Skill instructions cover call conditions, parameters, evidence interpretation,
and refresh rules. Generic contribution procedures follow Codex and repository
instructions. Removed lifecycle tools are not hidden inside discovery or assessment.
Updating the source skill does not refresh independently copied installations or
publish a cloud environment configuration.

## Legacy standalone skill

The installable product skill lives at
[`skills/issue-finder/SKILL.md`](../skills/issue-finder/SKILL.md). Its whole package is:

```text
skills/issue-finder/
  SKILL.md
  scripts/issue_finder.py
  agents/openai.yaml
```

Use the [root README installation instructions](../README.md#legacy-standalone-skill)
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

The independent legacy helper does not reuse CLI search, state, or result
handling. Its validation scope is separate from the Codex CLI version 2 contract.
Architecture documents for handoff/dispatch describe separate legacy workflows.

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

## Legacy handoff generated context (not an installable entry skill)

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
