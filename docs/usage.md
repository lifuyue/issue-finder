# Issue Finder Usage Guide

Issue Finder is a local-first task preparation tool for developers who use coding agents. The root README stays short; this guide keeps the operational details for installing, configuring, and running the CLI.

## Workflow

```text
Discover good first issues
  -> Rank with local heuristics
  -> Prepare repository workspace
  -> Run fixed low-risk preparation probes
  -> Generate handoff, policy, probe, event, and context artifacts
  -> Store the task in the local inbox
  -> Generate a daily report
```

`handoff.json` is the canonical output. `handoff.md` is the human-readable summary. `agent-policy.json`, `probe.json`, `prepare-events.jsonl`, `codex.md`, and `context/*.md` give downstream coding agents a safer starting point. See [Agent-Safe Preparation Runtime](./agent-safe-preparation-runtime.md) for the full artifact model.

## Requirements

- Rust toolchain and Cargo
- Git
- GitHub Personal Access Token

Optional:

- GitHub CLI (`gh`), useful for reusing an existing GitHub token
- OpenAI-compatible API key, used only when optional LLM summaries are enabled

## Installation

Install the published crate:

```bash
cargo install issue-finder
issue-finder --help
```

When working from this checkout, use Cargo directly:

```bash
cargo run -- --help
```

## GitHub Token

Issue Finder uses the GitHub REST API to discover issues and read repository metadata. Local use only needs read access.

You can enter a token during `issue-finder init`, or provide one through the environment:

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

Issue Finder does not need GitHub write permissions.

## Common Commands

Initialize local configuration and directories:

```bash
issue-finder init
```

Ask your main coding agent to draft profile settings from local Agent indexes and project manifests:

```bash
issue-finder profile bootstrap --json
```

Check local readiness:

```bash
issue-finder doctor
```

Discover and rank candidate issues:

```bash
issue-finder scout --limit 10
issue-finder scout --refresh
```

Prepare a specific issue:

```bash
issue-finder prepare owner/repo#123
issue-finder prepare --url https://github.com/owner/repo/issues/123
```

Read handoff output:

```bash
issue-finder handoff <inbox-id> --print
issue-finder handoff <inbox-id> --json
```

Manage local inbox items:

```bash
issue-finder inbox
issue-finder inbox --json
issue-finder inbox archive <inbox-id>
issue-finder inbox done <inbox-id>
```

Run the daily preparation flow:

```bash
issue-finder daily --top 3
issue-finder daily --refresh
issue-finder report
issue-finder report --date YYYY-MM-DD
```

## Command Reference

| Command | Purpose |
| --- | --- |
| `issue-finder init` | Create local config and Issue Finder state directories |
| `issue-finder profile bootstrap --json` | Scan supported local Agent indexes and project manifests, then print a profile bootstrap report |
| `issue-finder doctor` | Check Git, GitHub auth, config, directory permissions, platform, and optional LLM status |
| `issue-finder scout --limit 10` | Discover and rank good-first-issue candidates |
| `issue-finder scout --refresh` | Ignore the local GitHub issue cache and request fresh data |
| `issue-finder scout --json` | Print ranked candidates as JSON |
| `issue-finder prepare owner/repo#123` | Prepare one issue and write it to the inbox |
| `issue-finder prepare --url <url>` | Prepare one issue from a GitHub issue URL |
| `issue-finder handoff <id>` | Display an existing handoff |
| `issue-finder handoff <id> --print` | Print human-readable `handoff.md` |
| `issue-finder handoff <id> --json` | Print canonical `handoff.json` |
| `issue-finder inbox` | List local inbox items |
| `issue-finder inbox archive <id>` | Mark an inbox item as archived |
| `issue-finder inbox done <id>` | Mark an inbox item as done |
| `issue-finder daily --top 3` | Scout, prepare Top N issues, and write a daily report |
| `issue-finder report` | Display today's report |
| `issue-finder report --date YYYY-MM-DD` | Display a report for a specific date |

## Local State Directory

Issue Finder stores local state under `~/.issue-finder` by default:

```text
~/.issue-finder/
  config.toml
  cache/
    github-issues.json
  workspaces/
    owner__repo/
  inbox/
    index.json
    YYYY-MM-DD-owner__repo-123/
      issue.json
      workspace.json
      handoff.json
      handoff.md
      codex.md
      agent-policy.json
      probe.json
      prepare-events.jsonl
      context/
        entry.md
        safety.md
        probe.md
        value.md
        issue.md
        repo.md
        validation.md
      .agents/
        skills/
          issue-finder/
            SKILL.md
            refs.json
  reports/
    YYYY-MM-DD.md
```

Use `ISSUE_FINDER_HOME` for isolated testing or demos:

```bash
ISSUE_FINDER_HOME=/tmp/issue-finder-demo issue-finder doctor
```

## Configuration

`~/.issue-finder/config.toml`:

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

If `llm.api_key_env` is set, Issue Finder reads the LLM key from that environment variable instead of `llm.api_key`.

### Profile Bootstrap

`issue-finder profile bootstrap --json` scans supported low-risk local Agent sources under the operating system home directory, such as Codex session indexes, history indexes, memories, and conservative Claude/Cursor index-style files. It then reads root project manifests for discovered working directories and emits a structured report with active projects, tech stack evidence, keyword evidence, recent task themes, and a recommended `[profile]` draft.

The command does not write `config.toml`. A main Agent or human should review the report, remove noise, confirm preferences, and then update `[profile]`.

By default it does not read complete conversation bodies, system prompts, tool output, diffs, patches, shell output, or secrets. The scan is complete for supported source files and root manifests, but the conversation body mode remains disabled.

## Handoff Output

`handoff.json` contains:

- Issue metadata
- Workspace path, default branch, Issue Finder branch, and dirty status
- Candidate files
- Suggested validation commands
- Warnings
- Progressive context pack references
- Agent policy manifest
- Safe probe pack
- Preparation readiness score
- Instructions for a coding agent or human contributor
- Optional LLM summary status

`handoff.md` is a short readable summary that points back to `handoff.json`, `agent-policy.json`, and `probe.json`.

`codex.md` is the shortest entrypoint to give to Codex. It points to `context/entry.md`, `context/safety.md`, and `context/probe.md` first, then defers value, issue, repo, and validation context until those details are needed.

`agent-policy.json` is an agent-facing safety contract. It marks low-risk probe commands as allowed, validation commands as requiring user approval, and destructive or out-of-bound actions as forbidden. It is not an operating system sandbox.

`probe.json` records fixed preparation probes and static repository facts, including workspace dirty state, current branch, origin URL, package managers, detected package scripts, agent instruction files, validation candidates, probe warnings, and truncation or timeout details.

Runtime topic docs:

- [Sandbox & approvals](./sandbox.md)
- [Execution policy](./execpolicy.md)
- [Safe probes](./safe-probes.md)
- [Skills and context pack](./skills.md)

## Safety Boundary

Issue Finder is intentionally conservative.

Allowed:

- Read GitHub issue and repository metadata
- Clone or fetch repositories
- Create or checkout a local Issue Finder branch
- Scan repository files within a limited scope
- Run fixed low-risk probes such as `git status --porcelain`, `git branch --show-current`, `git ls-files`, and package script metadata reads
- Write Issue Finder state under `~/.issue-finder` or `ISSUE_FINDER_HOME`

Not allowed:

- Modify target repository source
- Automatically run target repository validation commands
- Install dependencies
- Commit
- Push
- Create pull requests
- Reset, clean, or delete workspaces

Issue Finder writes suggested validation commands into the handoff package but does not run them automatically.
Validation, build, lint, install, network-heavy, and project-defined script commands are classified as requiring approval or forbidden for downstream agents.
