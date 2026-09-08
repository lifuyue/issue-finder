# Issue Finder Skill Integration

The installable product skill lives at
[`skills/issue-finder/SKILL.md`](../skills/issue-finder/SKILL.md). Its whole package is:

```text
skills/issue-finder/
  SKILL.md
  scripts/issue_finder.py
  agents/openai.yaml
```

Use the [root README installation instructions](../README.md#codex-skill-install-and-use)
to copy or symlink this directory into `~/.agents/skills/issue-finder`, or copy it
into a target repository's `.agents/skills/issue-finder`. Copying only `SKILL.md`
is insufficient: the workflow needs its bundled script. The package is standalone
and can run after the source checkout is removed when installed by copying.
It does not call the Rust CLI or read `~/.issue-finder`.

## Ownership and execution

The current Codex session owns semantic selection, feasibility review, code edits,
debugging, final diff review, and reporting. The helper owns only deterministic
GitHub reads, workspace operations, checks, and result collection. There is one
skill, three commands, and at most one temporary task JSON per prepared workspace.
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

## Migration and validation scope

This integration implements the standalone skill path. The existing Rust runtime
is retained as a comparison baseline, not an adapter or fallback. Removing the
old dispatch/memory/MCP/handoff runtime is a later migration step after real-world
workflow and quality comparisons; it is not a prerequisite for installing this
skill. Rust CLI architecture documents describe that separate implementation.

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
these tests does not establish that parity or warrant deleting the Rust baseline.
The [initial read-only smoke report](./recommendation-evals/2026-08-31-skill-native-smoke/report.md)
records observed GitHub behavior, the failures turned into fixtures, and its limits.

## Existing Rust CLI generated context (not the installable skill)

The Rust CLI still writes a per-item context entry for its handoff workflow.
Do not install these generated files in place of the stable package above.

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
