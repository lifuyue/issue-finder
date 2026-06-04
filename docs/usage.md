# Patchbay CLI Usage Guide

Patchbay is a local-first task preparation tool for developers who use coding agents. The root README stays short; this guide keeps the operational details for installing, configuring, and running the CLI.

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

Build the debug binary:

```bash
cargo build
target/debug/patchbay --help
```

Install from the current checkout:

```bash
cargo install --path .
patchbay --help
```

## GitHub Token

Patchbay uses the GitHub REST API to discover issues and read repository metadata. Local use only needs read access.

You can enter a token during `patchbay init`, or provide one through the environment:

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

Patchbay does not need GitHub write permissions.

## Common Commands

Initialize local configuration and directories:

```bash
patchbay init
```

Check local readiness:

```bash
patchbay doctor
```

Discover and rank candidate issues:

```bash
patchbay scout --limit 10
patchbay scout --refresh
```

Prepare a specific issue:

```bash
patchbay prepare owner/repo#123
patchbay prepare --url https://github.com/owner/repo/issues/123
```

Read handoff output:

```bash
patchbay handoff <inbox-id> --print
patchbay handoff <inbox-id> --json
```

Manage local inbox items:

```bash
patchbay inbox
patchbay inbox --json
patchbay inbox archive <inbox-id>
patchbay inbox done <inbox-id>
```

Run the daily preparation flow:

```bash
patchbay daily --top 3
patchbay daily --refresh
patchbay report
patchbay report --date YYYY-MM-DD
```

## Command Reference

| Command | Purpose |
| --- | --- |
| `patchbay init` | Create local config and Patchbay state directories |
| `patchbay doctor` | Check Git, GitHub auth, config, directory permissions, platform, and optional LLM status |
| `patchbay scout --limit 10` | Discover and rank good-first-issue candidates |
| `patchbay scout --refresh` | Ignore the local GitHub issue cache and request fresh data |
| `patchbay scout --json` | Print ranked candidates as JSON |
| `patchbay prepare owner/repo#123` | Prepare one issue and write it to the inbox |
| `patchbay prepare --url <url>` | Prepare one issue from a GitHub issue URL |
| `patchbay handoff <id>` | Display an existing handoff |
| `patchbay handoff <id> --print` | Print human-readable `handoff.md` |
| `patchbay handoff <id> --json` | Print canonical `handoff.json` |
| `patchbay inbox` | List local inbox items |
| `patchbay inbox archive <id>` | Mark an inbox item as archived |
| `patchbay inbox done <id>` | Mark an inbox item as done |
| `patchbay daily --top 3` | Scout, prepare Top N issues, and write a daily report |
| `patchbay report` | Display today's report |
| `patchbay report --date YYYY-MM-DD` | Display a report for a specific date |

## Local State Directory

Patchbay stores local state under `~/.patchbay` by default:

```text
~/.patchbay/
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
          patchbay-cli/
            SKILL.md
            refs.json
  reports/
    YYYY-MM-DD.md
```

Use `PATCHBAY_HOME` for isolated testing or demos:

```bash
PATCHBAY_HOME=/tmp/patchbay-demo patchbay doctor
```

## Configuration

`~/.patchbay/config.toml`:

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

If `llm.api_key_env` is set, Patchbay reads the LLM key from that environment variable instead of `llm.api_key`.

## Handoff Output

`handoff.json` contains:

- Issue metadata
- Workspace path, default branch, Patchbay branch, and dirty status
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

Patchbay is intentionally conservative.

Allowed:

- Read GitHub issue and repository metadata
- Clone or fetch repositories
- Create or checkout a local Patchbay branch
- Scan repository files within a limited scope
- Run fixed low-risk probes such as `git status --porcelain`, `git branch --show-current`, `git ls-files`, and package script metadata reads
- Write Patchbay state under `~/.patchbay` or `PATCHBAY_HOME`

Not allowed:

- Modify target repository source
- Automatically run target repository validation commands
- Install dependencies
- Commit
- Push
- Create pull requests
- Reset, clean, or delete workspaces

Patchbay writes suggested validation commands into the handoff package but does not run them automatically.
Validation, build, lint, install, network-heavy, and project-defined script commands are classified as requiring approval or forbidden for downstream agents.
