# Issue Finder Rust Design

## Summary

Issue Finder is a Rust rewrite inspired by the current OpenMeta project, but it uses a new brand and a narrower product boundary. Issue Finder is a local-first open-source task preparation tool for developers who use coding agents. It does not compete with Codex, Cursor, Claude Code, Cline, or similar tools. It prepares a clear task package before those tools begin work.

The command name is `issue-finder`. The repository and package name should be `issue-finder`.

Current implementation note: this document records the first Rust product boundary. The original handoff model has since been extended by the agent-safe preparation runtime and the staged agent dispatch control plane. See [`../agent-safe-preparation-runtime.md`](../agent-safe-preparation-runtime.md) for the current prepared artifact structure and [`superpowers/specs/2026-06-18-agent-dispatch-control-plane-design.md`](./superpowers/specs/2026-06-18-agent-dispatch-control-plane-design.md) for the dispatch architecture.

## Product Positioning

Issue Finder is a local context and handoff layer for open-source contribution work. Its job is to find suitable `good first issue` tasks, prepare local repository context, create a structured handoff payload, store that payload in a local inbox, and produce a daily task report.

The first version optimizes for a deterministic local workflow:

```text
Discover good first issues
  -> Rank with local heuristics
  -> Prepare repository workspace
  -> Generate handoff, policy, probe, event, and context artifacts
  -> Store the task in the local inbox
  -> Generate a daily report.md
```

`handoff.json` is the canonical prepared inbox artifact. `handoff.md` is a lightweight human-readable summary. Newer prepared items also include `codex.md`, `agent-policy.json`, `probe.json`, `prepare-events.jsonl`, generated context files, and a local `issue-finder` skill. Dispatch state stores handoff and task package payloads as durable artifacts rather than replacing the inbox format in the first migration.

## Goals

- Discover GitHub issues labeled `good first issue` or `good-first-issue`.
- Rank discovered issues with deterministic local heuristics.
- Prepare a local workspace for selected issues.
- Generate a structured `handoff.json` that a coding agent can consume.
- Generate a short `handoff.md` for human review.
- Maintain a local inbox of prepared handoffs.
- Generate a local Markdown daily report.
- Support optional OpenAI-compatible LLM enhancement without making LLMs required.
- Keep the first version safe by avoiding code edits, test execution, commits, pushes, and PR creation.

## Historical Non-Goals

- Do not implement a general coding agent.
- The original handoff-only version did not start Codex, Cursor, Claude Code, Cline, or other coding agents. The later dispatch control plane adds approval-gated native Codex session dispatch as a separate runtime boundary.
- Do not generate or apply patches.
- Do not modify target repository source files.
- Do not run validation commands automatically.
- Do not commit, push, or open pull requests.
- Do not implement MCP server support in the first version.
- Do not implement complex LLM provider profiles.
- Do not implement repo memory or user memory recommendation learning loops in the first version.
- Do not publish artifacts to a remote private repository.
- Do not implement an automation enable/disable state machine in the first version.

## Command Surface

The first version has a complete but restrained command surface:

```bash
issue-finder init
issue-finder scout
issue-finder prepare owner/repo#123
issue-finder handoff <inbox-id>
issue-finder inbox
issue-finder daily
issue-finder report
issue-finder doctor
```

### `issue-finder init`

Initializes local configuration and creates the local Issue Finder directory structure.

It captures:

- GitHub token.
- Optional GitHub username.
- User profile keywords and tech stack.
- Daily Top N setting.
- Optional OpenAI-compatible LLM settings.

The config file is written to:

```text
~/.issue-finder/config.toml
```

### `issue-finder scout`

Discovers and ranks `good first issue` tasks.

Example usage:

```bash
issue-finder scout --limit 20
issue-finder scout --refresh
```

`scout` only discovers and displays candidates. It does not clone repositories and does not generate handoff payloads.

### `issue-finder prepare`

Prepares one issue and writes it to the inbox.

Example usage:

```bash
issue-finder prepare owner/repo#123
issue-finder prepare --url https://github.com/owner/repo/issues/123
```

The command:

- Fetches issue details if needed.
- Clones or fetches the repository workspace.
- Creates or reuses a Issue Finder branch.
- Scans repository structure.
- Detects candidate files.
- Detects suggested validation commands.
- Builds `handoff.json`.
- Renders `handoff.md`.
- Upserts the inbox index.

### `issue-finder handoff`

Displays or prints an existing handoff.

Example usage:

```bash
issue-finder handoff <inbox-id>
issue-finder handoff <inbox-id> --json
issue-finder handoff <inbox-id> --print
```

The JSON output is the authoritative payload.

### `issue-finder inbox`

Lists the local inbox and supports light status management.

Example usage:

```bash
issue-finder inbox
issue-finder inbox --json
issue-finder inbox archive <inbox-id>
issue-finder inbox done <inbox-id>
```

First-version inbox statuses are:

```text
ready
prepare_failed
archived
done
```

### `issue-finder daily`

Runs the daily task preparation flow.

Example usage:

```bash
issue-finder daily
issue-finder daily --top 5
issue-finder daily --refresh
```

The command:

```text
scout
  -> select Top N issues not already in inbox
  -> prepare each issue
  -> write or update inbox entries
  -> write reports/YYYY-MM-DD.md
```

Single-issue failures do not stop the full daily run. The report records both successful and failed preparations.

### `issue-finder report`

Displays local daily reports.

Example usage:

```bash
issue-finder report
issue-finder report --date 2026-06-02
```

Reports are Markdown files. They are a simple local knowledge base, not a publishing surface.

### `issue-finder doctor`

Checks local readiness.

It verifies:

- Git availability.
- GitHub token presence and basic validity.
- Issue Finder config existence.
- Issue Finder directory permissions.
- Workspace, inbox, cache, and report paths.
- Optional LLM reachability when LLM is enabled.
- Platform details relevant to future scheduler support.

## Local State Layout

Issue Finder stores local state under `~/.issue-finder`.

```text
~/.issue-finder/
  config.toml
  cache/
    github-issues.json
  workspaces/
    owner__repo/
  inbox/
    index.json
    2026-06-02-owner__repo-123/
      issue.json
      workspace.json
      handoff.json
      handoff.md
      codex.md
      agent-policy.json
      probe.json
      prepare-events.jsonl
      context/
      .agents/
  reports/
    2026-06-02.md
```

`ISSUE_FINDER_HOME` can override `~/.issue-finder` for tests, development, and advanced users.

## Configuration

The first version uses a single TOML configuration file.

```toml
[github]
token = ""
username = ""

[profile]
tech_stack = ["Rust", "TypeScript"]
keywords = ["cli", "developer-tools"]

[daily]
top_n = 5

[llm]
enabled = false
base_url = "https://api.openai.com/v1"
api_key = ""
api_key_env = ""
model = "gpt-4o-mini"
```

LLM settings are intentionally simple. The first version does not support provider presets, named profiles, or complex custom headers. If `api_key_env` is set, Issue Finder reads the API key from that environment variable instead of requiring it in the config file.

## Issue Discovery

The first version discovers issues through the GitHub REST API using search queries for:

```text
label:"good first issue"
label:"good-first-issue"
```

Discovery filters:

- Open issues only.
- Exclude pull requests.
- Exclude locked issues.
- Exclude assigned issues when possible.
- Exclude archived repositories when possible.

Discovery results are cached in:

```text
~/.issue-finder/cache/github-issues.json
```

The default discovery cache TTL is 10 minutes. `--refresh` bypasses the cache.

## Local Scoring

The first version uses deterministic local scoring. It does not implement complex agent suitability scoring.

Scoring inputs:

- Issue title keyword matches.
- Issue body keyword matches.
- Repository name and description matches.
- User profile `tech_stack`.
- User profile `keywords`.
- Issue freshness.
- Repository stars.
- Presence of actionable signals such as file paths, reproduction steps, expected behavior, or actual behavior.

The scoring module returns a ranked list and a short explanation for each score.

## Workspace Preparation

Workspaces are stored under:

```text
~/.issue-finder/workspaces/owner__repo
```

Preparation steps:

- Clone the repository if no local workspace exists.
- Fetch the default remote if the workspace already exists.
- Detect the default branch.
- Create or reuse a branch named like `issue-finder/123-short-title`.
- Detect whether the workspace is dirty.
- Scan repository files.
- Detect candidate files.
- Detect suggested validation commands.

If the workspace is dirty, Issue Finder does not reset or overwrite it. The handoff payload records a warning.

Git operations should use a small Rust wrapper around the local `git` CLI for the first version. This keeps behavior close to what users expect from their own Git configuration, SSH setup, credential helpers, and proxy settings.

## Repo Scan

Repo scanning is lightweight and bounded.

Excluded directories:

```text
.git
node_modules
target
dist
build
.next
vendor
coverage
```

Scan constraints:

- Maximum discovered file count.
- Maximum single-file read size.
- No binary file parsing.
- No dependency installation.
- No command execution.

Candidate files are selected by matching issue title and body terms against file paths and limited snippets.

Suggested validation command detection:

```text
Cargo.toml       -> cargo test
package.json     -> npm test, pnpm test, yarn test, or bun test based on lockfile
pyproject.toml   -> pytest
go.mod           -> go test ./...
Makefile         -> make test when a test target is present
```

Issue Finder only suggests these commands in the handoff. It does not run them in the first version.

## Handoff Payload

`handoff.json` is the canonical payload.

Current prepared items add `context_pack`, `agent_policy`, `probe_pack`, `readiness`, `value_assessment`, and `evidence_pack` to the first-version payload. The additive fields are documented in [`../agent-safe-preparation-runtime.md`](../agent-safe-preparation-runtime.md).

Example shape:

```json
{
  "version": 1,
  "kind": "issue_finder_handoff",
  "id": "2026-06-02-owner__repo-123",
  "created_at": "2026-06-02T10:00:00Z",
  "issue": {
    "repo_full_name": "owner/repo",
    "number": 123,
    "title": "Fix accessible button label",
    "body": "...",
    "labels": ["good first issue"],
    "url": "https://github.com/owner/repo/issues/123",
    "updated_at": "2026-06-02T09:00:00Z"
  },
  "workspace": {
    "path": "/Users/example/.issue-finder/workspaces/owner__repo",
    "default_branch": "main",
    "branch": "issue-finder/123-fix-accessible-button-label",
    "dirty": false
  },
  "context": {
    "candidate_files": [
      {
        "path": "src/button.rs",
        "reason": "Path matched issue terms"
      }
    ],
    "validation_commands": [
      {
        "command": "cargo test",
        "reason": "Detected Cargo.toml"
      }
    ],
    "warnings": []
  },
  "instructions": {
    "goal": "Investigate and fix the issue with a minimal patch.",
    "suggested_start": [
      "Read the issue body",
      "Inspect candidate files",
      "Make the smallest targeted change"
    ],
    "constraints": [
      "Keep changes minimal",
      "Do not open a PR automatically",
      "Do not overwrite unrelated local changes"
    ],
    "expected_output": [
      "Patch in local workspace",
      "Validation result",
      "PR summary draft"
    ]
  },
  "llm_enhancement": {
    "status": "disabled",
    "summary": null,
    "warnings": []
  }
}
```

`handoff.md` is a compact summary that points to the JSON payload:

```md
# Handoff: owner/repo#123

- JSON payload: ./handoff.json
- Workspace: /Users/example/.issue-finder/workspaces/owner__repo
- Branch: issue-finder/123-fix-accessible-button-label
- Suggested files: src/button.rs
- Suggested validation: cargo test

## Goal

Investigate and fix the issue with a minimal patch.
```

## Inbox

`inbox/index.json` stores task metadata and points to each task directory.

Example:

```json
{
  "items": [
    {
      "id": "2026-06-02-owner__repo-123",
      "repo_full_name": "owner/repo",
      "issue_number": 123,
      "title": "Fix accessible button label",
      "score": 82,
      "status": "ready",
      "handoff_json_path": "/Users/example/.issue-finder/inbox/2026-06-02-owner__repo-123/handoff.json",
      "handoff_md_path": "/Users/example/.issue-finder/inbox/2026-06-02-owner__repo-123/handoff.md",
      "created_at": "2026-06-02T10:00:00Z"
    }
  ]
}
```

The inbox index is an index only. Full issue, workspace, and handoff content live in the task directory.

The `instructions.expected_output` field describes what the downstream coding agent or user should produce after accepting the handoff. It is not a list of actions Issue Finder performs in the first version.

## Daily Report

Daily reports are written to:

```text
~/.issue-finder/reports/YYYY-MM-DD.md
```

Report content:

- Run timestamp.
- Discovery count.
- Prepared handoff count.
- Failed preparation count.
- Prepared handoff list with scores and paths.
- Failed issue list with reasons.
- Recommended top 1 to 3 tasks for the day.

Reports are local Markdown knowledge-base entries.

## Optional LLM Enhancement

The LLM is optional and never controls the core workflow.

Without LLM:

- Issue Finder still discovers issues.
- Issue Finder still prepares workspaces.
- Issue Finder still generates complete `handoff.json`.
- Issue Finder still generates `handoff.md` and `report.md`.

With LLM enabled:

- Issue Finder may improve issue summary.
- Issue Finder may rewrite the goal.
- Issue Finder may improve suggested start steps.
- Issue Finder may add likely risk notes.
- Issue Finder may make the daily report more readable.

LLM failures do not block handoff generation. Failures are recorded in `llm_enhancement.status = "failed"` and in warnings.

The LLM must not decide:

- Git operations.
- File write locations.
- Workspace branch names.
- Validation command detection.
- Inbox state.
- Whether a task is safe to modify.

## Rust Architecture

The first version should be a single Rust crate. It can be split into a workspace later if MCP, server mode, or adapters are added.

Suggested modules:

```text
src/
  main.rs
  cli.rs
  config.rs
  paths.rs
  github.rs
  scoring.rs
  workspace.rs
  repo_scan.rs
  handoff.rs
  inbox.rs
  report.rs
  llm.rs
  doctor.rs
  errors.rs
```

Suggested dependencies:

```text
clap
serde
serde_json
toml
reqwest
tokio
anyhow
thiserror
chrono
dirs
walkdir
tracing
```

Git operations should be implemented as a wrapper around the local `git` CLI in the first version.

## Error Handling

Issue Finder should preserve clear evidence and avoid stopping a whole batch when one task fails.

Rules:

- GitHub rate limit errors are shown explicitly.
- `scout` can fall back to fresh cache when available.
- LLM failures degrade to deterministic template output.
- Clone or fetch failure marks the task as `prepare_failed`.
- Repo scan failure still allows handoff generation when enough issue and workspace data exists.
- Dirty workspace never triggers reset or overwrite.
- `daily` continues after single-task failure.
- File write failure is a hard failure for that specific output.

File writes for index and payload files should use a temporary file followed by atomic rename where practical.

## Safety Boundary

The first version only writes Issue Finder-owned local state and prepares local Git workspaces.

Allowed actions:

- Read GitHub issue and repository metadata.
- Clone repositories.
- Fetch repositories.
- Create or checkout Issue Finder branches.
- Scan repository files under bounded limits.
- Write files under `~/.issue-finder`.

Disallowed actions:

- Modify target repository source files.
- Run arbitrary repository commands.
- Install dependencies.
- Commit.
- Push.
- Open PRs.
- Reset, clean, or delete workspaces.

## Testing Strategy

Unit tests:

- Issue reference parsing.
- Issue URL parsing.
- GitHub search query construction.
- Local scoring behavior.
- Repo scan directory exclusions.
- Validation command detection.
- Handoff JSON generation.
- Handoff Markdown rendering.
- Inbox index upsert.
- Report Markdown rendering.
- LLM fallback behavior.

Integration tests:

- Use `ISSUE_FINDER_HOME` with a temporary directory.
- Use a local Git repository to test workspace preparation.
- Mock GitHub HTTP responses for `scout`.
- Verify `daily` continues when one preparation fails and another succeeds.
- Verify `daily` writes both inbox entries and report entries.

## Success Criteria

The first version is successful when a user can run:

```bash
issue-finder init
issue-finder daily --top 3
issue-finder inbox
issue-finder handoff <id> --json
```

and receive:

- Up to 3 prepared handoffs.
- A canonical `handoff.json` for each prepared task.
- A short `handoff.md` for each prepared task.
- A daily report at `~/.issue-finder/reports/YYYY-MM-DD.md`.
- A workflow that succeeds without LLM configuration.
- Better summaries when LLM is configured, without LLM becoming required.
- No source-code edits, commits, pushes, PRs, or test execution performed by Issue Finder.

## Implementation Notes

The first implementation should build deterministic core behavior before adding optional LLM enhancement:

1. CLI, config, and path setup.
2. GitHub discovery.
3. Local scoring.
4. Workspace preparation.
5. Repo scan.
6. `handoff.json`.
7. `handoff.md`.
8. Inbox.
9. Daily report.
10. Doctor.
11. Optional LLM enhancement.

This sequence keeps the project usable even before any AI-dependent behavior is added.
