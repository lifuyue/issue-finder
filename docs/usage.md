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
  -> Track optional dispatch state, session links, and result artifacts
  -> Project local candidate/task board state for queries
  -> Generate a daily report
```

`handoff.json` remains the canonical prepared handoff artifact. In the dispatch control plane, prepared handoffs and future task packages are durable artifacts tracked alongside runs, session links, approvals, events, and result artifacts. `handoff.md` is the human-readable summary. `agent-policy.json`, `probe.json`, `prepare-events.jsonl`, `codex.md`, and `context/*.md` give downstream coding agents a safer starting point. See [Agent-Safe Preparation Runtime](./agent-safe-preparation-runtime.md) for the full artifact model.

## Requirements

- Rust toolchain and Cargo
- Git
- GitHub Personal Access Token with read access for discovery

Optional:

- GitHub CLI (`gh`), useful for reusing an existing GitHub token
- GitHub issue write permission, needed only if you explicitly approve and post GitHub comments through dispatch projection commands
- OpenAI-compatible API key, used only when optional LLM summaries are enabled
- Codex CLI, required only when using the experimental native Codex app-server adapter

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

Issue Finder uses the GitHub REST API to discover issues and read repository metadata. Discovery, assessment, preparation, and local dispatch state only need read access.

You can enter a token during `issue-finder init`, or provide one through the environment:

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

GitHub write permission is optional. It is used only by `issue-finder dispatch github post <interaction-id>` after a local interaction policy decision creates a draft and a human approves the `github_post` approval request.

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

Check JSON tool readiness before an agent calls discovery or prepare:

```bash
issue-finder tools list
issue-finder tools call issue-finder.status --arguments '{}'
```

Discover and rank candidate issues:

```bash
issue-finder scout --limit 10
issue-finder scout --repo owner/repo --limit 10
issue-finder scout --refresh
issue-finder assess owner/repo#123
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

Inspect local dispatch state:

```bash
issue-finder agents list
issue-finder agents capabilities codex
issue-finder agents probe codex
issue-finder sessions list --agent codex
issue-finder sessions sync --agent codex --limit 20
issue-finder sessions search --issue owner/repo#123
issue-finder sessions read <session-link-id>
issue-finder sessions replay <session-link-id>
issue-finder sessions rename <session-link-id> --name "issue-finder: owner/repo#123 - short title"
issue-finder sessions fork <session-link-id>
issue-finder sessions archive <session-link-id>
issue-finder sessions approve <approval-request-id>
issue-finder sessions reject <approval-request-id>
issue-finder dispatch package import-handoff <inbox-id>
issue-finder dispatch review list
issue-finder dispatch review show <approval-request-id>
issue-finder dispatch review approve <approval-request-id>
issue-finder dispatch review reject <approval-request-id> --reason "..."
issue-finder dispatch owner/repo#123 --agent codex --new-session
issue-finder dispatch owner/repo#123 --agent codex --session <session-link-or-native-id>
issue-finder dispatch approve <run-id>
issue-finder dispatch execute <run-id>
issue-finder dispatch a2a export owner/repo#123
issue-finder dispatch a2a approve <approval-request-id>
issue-finder dispatch a2a reject <approval-request-id>
issue-finder dispatch a2a import-result <run-id> --path ./fix_result.json --status completed
issue-finder dispatch github draft-tracking owner/repo#123
issue-finder dispatch github draft-final <run-id>
issue-finder dispatch github approve <interaction-id>
issue-finder dispatch github reject <interaction-id>
issue-finder dispatch github post <interaction-id>
issue-finder dispatch github retry <interaction-id>
issue-finder dispatch github list --issue owner/repo#123
issue-finder dispatch status <run-id>
issue-finder dispatch events <run-id>
issue-finder dispatch timeline <run-id>
issue-finder dispatch trace <run-id>
issue-finder dispatch artifacts <run-id>
```

`dispatch package import-handoff` creates an `issue_review` approval request and stores the handoff/profile snapshot as artifacts. It does not create an `IssueTaskPackage` until `dispatch review approve <approval-request-id>` resolves the review. Direct dispatch, A2A, and GitHub projection commands may auto-import a matching ready inbox handoff, but they return `pending_issue_review` until that approval creates the package.

Manage local inbox items:

```bash
issue-finder inbox
issue-finder inbox --json
issue-finder inbox archive <inbox-id>
issue-finder inbox done <inbox-id>
```

Record or inspect recommendation feedback:

```bash
issue-finder feedback read owner/repo#123
issue-finder feedback dismiss owner/repo#123
issue-finder feedback restore owner/repo#123
issue-finder feedback show owner/repo#123
```

Inspect and control contribution memory:

```bash
issue-finder memory status
issue-finder memory events --issue owner/repo#123
issue-finder memory recall --issue owner/repo#123 --kind scout-ranking
issue-finder memory dreams list
issue-finder memory dreams show <dream-id>
issue-finder memory hints list
issue-finder memory hints approve <hint-id>
issue-finder memory hints reject <hint-id>
issue-finder memory hints pin <hint-id>
issue-finder memory hints deprioritize <hint-id>
issue-finder memory suppress --scope repo:owner/repo
issue-finder memory tombstone <event-or-node-or-hint-id>
issue-finder memory dream --scope global
issue-finder memory eval --offline --output <dir>
```

Run the daily preparation flow:

```bash
issue-finder daily --top 3
issue-finder daily --repo owner/repo --top 3
issue-finder daily --refresh
issue-finder report
issue-finder report --date YYYY-MM-DD
```

Run deterministic evaluation workflows:

```bash
issue-finder eval recommendation --offline --output <dir>
issue-finder eval agent-loop --offline --output <dir>
```

## Command Reference

| Command | Purpose |
| --- | --- |
| `issue-finder init` | Create local config and Issue Finder state directories |
| `issue-finder profile bootstrap --json` | Scan supported local Agent indexes and project manifests, then print a profile bootstrap report |
| `issue-finder doctor` | Check Git, GitHub auth, config, directory permissions, platform, and optional LLM status |
| `issue-finder tools list` | Print the current Issue Finder JSON tool catalog |
| `issue-finder tools call issue-finder.status --arguments '{}'` | Return JSON config, token source, and GitHub auth diagnostics without printing tokens |
| `issue-finder scout --limit 10` | Discover and rank good-first-issue candidates |
| `issue-finder scout --repo owner/repo --limit 10` | Discover and rank candidates strictly within one repository |
| `issue-finder scout --refresh` | Ignore the local GitHub issue cache and request fresh data |
| `issue-finder scout --json` | Print ranked candidates as JSON |
| `issue-finder assess owner/repo#123` | Assess one issue without preparing workspace or handoff state |
| `issue-finder assess --url <url>` | Assess one issue from a GitHub issue URL |
| `issue-finder prepare owner/repo#123` | Prepare one issue and write it to the inbox |
| `issue-finder prepare --url <url>` | Prepare one issue from a GitHub issue URL |
| `issue-finder handoff <id>` | Display an existing handoff |
| `issue-finder handoff <id> --print` | Print human-readable `handoff.md` |
| `issue-finder handoff <id> --json` | Print canonical `handoff.json` |
| `issue-finder agents list` | List local execution agent profiles |
| `issue-finder agents capabilities codex` | List one agent's declared native capabilities; wired Codex app-server session capabilities are experimental, while unwired capabilities such as `stream_events`, `interrupt_run`, `review_mode`, and `open_pr` are reported as unsupported |
| `issue-finder agents probe codex` | Probe one agent adapter capability surface and cache the result |
| `issue-finder sessions list --agent codex` | List local links to native sessions for one agent |
| `issue-finder sessions sync --agent codex --limit 20` | Sync recent native Codex sessions into local session links |
| `issue-finder sessions search --issue owner/repo#123` | Search local session links for a GitHub issue |
| `issue-finder sessions read <session-link-id>` | Read a native session transcript into a local artifact |
| `issue-finder sessions replay <session-link-id>` | List normalized replay items for a local session link |
| `issue-finder sessions rename <session-link-id> --name <name>` | Create an approval request to rename a native session |
| `issue-finder sessions fork <session-link-id>` | Create an approval request to fork a native session into a new local session link |
| `issue-finder sessions archive <session-link-id>` | Create an approval request to archive a native session |
| `issue-finder sessions approve <approval-request-id>` | Approve and execute a pending native session mutation |
| `issue-finder sessions reject <approval-request-id>` | Reject a pending native session mutation |
| `issue-finder dispatch package import-handoff <id>` | Import an existing inbox handoff as an `issue_review` candidate and create an approval request |
| `issue-finder dispatch review list` | List pending and resolved issue review requests |
| `issue-finder dispatch review show <approval-request-id>` | Show one issue review request, including imported handoff/package evidence |
| `issue-finder dispatch review approve <approval-request-id>` | Approve one issue review and create the `IssueTaskPackage` v3 artifact |
| `issue-finder dispatch review reject <approval-request-id>` | Reject one issue review without dismissing the recommendation |
| `issue-finder dispatch owner/repo#123 --agent codex --new-session` | Create a pending dispatch approval without starting Codex; returns `pending_issue_review` first if the package has not been review-approved |
| `issue-finder dispatch owner/repo#123 --agent codex --session <session-link-or-native-id>` | Create a pending approval to continue an existing local session link or native session; returns `pending_issue_review` first if needed |
| `issue-finder dispatch propose owner/repo#123 --agent codex --new-session` | Explicit subcommand form for the same approval-gated dispatch proposal |
| `issue-finder dispatch approve <run-id>` | Resolve a pending dispatch approval and move the run to `approved` |
| `issue-finder dispatch reject <run-id>` | Reject a pending dispatch approval and cancel the run |
| `issue-finder dispatch execute <run-id>` | Connect to the run's native adapter and start the first turn after local approval |
| `issue-finder dispatch outcome record <run-id> --outcome <kind>` | Record a normalized terminal or blocked dispatch outcome |
| `issue-finder dispatch a2a export owner/repo#123` | Create a local A2A task artifact from the approved package v3 contract and an `a2a_send` approval request without network I/O; returns `pending_issue_review` first if needed |
| `issue-finder dispatch a2a approve <approval-request-id>` | Approve an outbound A2A task artifact for external use |
| `issue-finder dispatch a2a reject <approval-request-id>` | Reject an outbound A2A task artifact |
| `issue-finder dispatch a2a import-result <run-id> --path <file>` | Import a local A2A result file that satisfies the package outcome contract |
| `issue-finder dispatch github draft-tracking owner/repo#123` | Evaluate tracking-comment policy; default is `no_comment`, and allowed drafts create a local GitHub post approval; returns `pending_issue_review` first if needed |
| `issue-finder dispatch github draft-final <run-id>` | Evaluate final/clarification policy from the run's outcome and result artifact; only explicit suggested replies create a local GitHub post approval |
| `issue-finder dispatch github approve <interaction-id>` | Approve a drafted GitHub comment for posting |
| `issue-finder dispatch github reject <interaction-id>` | Reject a drafted GitHub comment |
| `issue-finder dispatch github post <interaction-id>` | Post an approved GitHub comment through the configured GitHub token |
| `issue-finder dispatch github retry <interaction-id>` | Retry a failed GitHub comment post |
| `issue-finder dispatch github list --issue owner/repo#123` | List local GitHub comment interactions for an issue task |
| `issue-finder dispatch status <run-id>` | Show one local dispatch run summary |
| `issue-finder dispatch events <run-id>` | List persisted events for a local dispatch run |
| `issue-finder dispatch timeline <run-id>` | Show the merged timeline for a local dispatch run |
| `issue-finder dispatch trace <run-id>` | Show diagnostic trace data for a local dispatch run |
| `issue-finder dispatch artifacts <run-id>` | List persisted artifacts for a local dispatch run |
| `issue-finder inbox` | List local inbox items |
| `issue-finder inbox archive <id>` | Mark an inbox item as archived |
| `issue-finder inbox done <id>` | Mark an inbox item as done |
| `issue-finder feedback read <issue>` | Mark an issue as read |
| `issue-finder feedback dismiss <issue>` | Hide an issue from future recommendation feed results |
| `issue-finder feedback restore <issue>` | Restore a done or dismissed issue to the recommendation feed |
| `issue-finder feedback show <issue>` | Show derived recommendation feedback state for an issue |
| `issue-finder memory status` | Show memory store status, state DB path, and decision-eligible hints |
| `issue-finder memory events --issue owner/repo#123` | List memory events without raw payloads |
| `issue-finder memory recall --issue owner/repo#123 --kind scout-ranking` | Recall contribution memory for an issue and query kind |
| `issue-finder memory dreams list` | List candidate and reviewed memory dreams |
| `issue-finder memory dreams show <dream-id>` | Show one memory dream and its hints |
| `issue-finder memory hints list` | List memory hints and decision-eligible hints |
| `issue-finder memory hints approve/reject/pin/deprioritize <hint-id>` | Review or adjust a memory hint |
| `issue-finder memory suppress --scope repo:owner/repo` | Suppress memory hints for one scope |
| `issue-finder memory tombstone <id>` | Tombstone a raw event, node, or hint |
| `issue-finder memory dream --scope global` | Run deterministic memory dreaming for a scope |
| `issue-finder memory eval --offline --output <dir>` | Run offline memory evaluation |
| `issue-finder daily --top 3` | Scout, prepare Top N issues, and write a daily report |
| `issue-finder daily --repo owner/repo --top 3` | Prepare Top N issues from one repository without cross-repo fallback |
| `issue-finder report` | Display today's report |
| `issue-finder report --date YYYY-MM-DD` | Display a report for a specific date |
| `issue-finder eval recommendation --offline --output <dir>` | Generate deterministic recommendation evaluation reports |
| `issue-finder eval recommendation --live --output <dir>` | Run the fixed six-profile live recommendation evaluation |
| `issue-finder eval agent-loop --offline --output <dir>` | Generate deterministic agent loop evaluation reports |

## Local State Directory

Issue Finder stores local state under `~/.issue-finder` by default:

```text
~/.issue-finder/
  config.toml
  state.sqlite3
  cache/
    github-issues.json
    discovery/
    enrichment/
    recommendation/
      scout-result/
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
  dispatch/
    dispatch.sqlite3
    artifacts/
  recommendation/
    events.jsonl
  reports/
    YYYY-MM-DD.md
```

`state.sqlite3` stores contribution memory tables. `dispatch/dispatch.sqlite3` stores dispatch/session/approval/GitHub projection state and dispatch artifacts live under `dispatch/artifacts/`.

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

`issue-finder profile bootstrap --json` scans supported low-risk local Agent sources under the operating system home directory, such as Codex session indexes, history indexes, rollout session JSONL files, archived session JSONL files, memories, and conservative Claude/Cursor index-style files. It then reads root project manifests for discovered working directories and emits a structured report with active projects, tech stack evidence, keyword evidence, recent task themes, and a recommended `[profile]` draft.

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
- Value assessment and recommendation assessment
- Evidence pack
- Approved memory context, when available
- Typed LLM confirmation status
- Instructions for a coding agent or human contributor
- Legacy optional LLM review fields, when present

`handoff.md` is a short readable summary that points back to `handoff.json`, `agent-policy.json`, and `probe.json`.

`codex.md` is the shortest entrypoint to give to Codex. It points to `context/entry.md`, `context/safety.md`, and `context/probe.md` first, then defers value, issue, repo, and validation context until those details are needed.

`agent-policy.json` is an agent-facing safety contract. It marks low-risk probe commands as allowed, validation commands as requiring user approval, and destructive or out-of-bound actions as forbidden. It is not an operating system sandbox.

`probe.json` records fixed preparation probes and static repository facts, including workspace dirty state, current branch, origin URL, package managers, detected package scripts, agent instruction files, validation candidates, probe warnings, and truncation or timeout details.

When dispatch state is used, `handoff.json` is imported as an issue review candidate first. Review approval writes a broader `IssueTaskPackage` v3 artifact. Package v3 is the execution-agent contract: it includes typed reproduction obligations, success criteria, change budget, environment contract, maintainer and interaction policy, session/resume context, and an expanded `fix_result.json` outcome contract. Issue-based dispatch and projection commands can import the matching ready inbox handoff automatically when local dispatch state does not exist yet, but they return `pending_issue_review` until `dispatch review approve <approval-request-id>` creates the package. The dispatch store records the package artifact path, user profile snapshot artifact, selected native session link, approval requests, typed `dispatch_events`, result artifacts, GitHub comment interactions, and GitHub interaction policy decisions including explicit `no_comment` and `no_reply` outcomes. The current CLI can manage linked native sessions, create and approve or reject issue reviews and dispatch proposals, execute approved runs through the native adapter, create local A2A task artifacts with explicit `a2a_send` approval before external use, import local A2A result artifacts that satisfy the package outcome contract, record normalized outcomes, and evaluate conservative GitHub interaction policy before drafting tracking, clarification, or final comments. Execution first performs local approval, package, and capability checks, then uses the isolated Codex app-server JSON-RPC adapter to start or resume a thread and send the first turn. The adapter starts or connects through `codex app-server daemon start` and `codex app-server proxy` by default, and records local startup metadata in agent capability details. Session read uses the same isolated adapter boundary and persists transcript artifacts and replay items locally. Session rename, fork, and archive are approval-gated mutations: the request creates a local approval first, and `sessions approve` performs the native mutation after approval. GitHub posting is a separate projection step: Issue Finder drafts comment bodies as local artifacts only when interaction policy finds clear public value, requires explicit approval, then posts through the configured GitHub token.

The candidate task board is a derived library-level read model over recommendation events, inbox items, and dispatch state. It is a query surface, not a persisted source of truth. Dispatch terminal outcomes remain visible as terminal board status even if an inbox item was marked done or archived; archive and dismiss feedback only affect display state. Reactivation is also projected locally and does not change recommendation feed score or memory ranking adjustments.

Runtime topic docs:

- [Sandbox & approvals](./sandbox.md)
- [Execution policy](./execpolicy.md)
- [Safe probes](./safe-probes.md)
- [Skills and context pack](./skills.md)
- [Superpowers design archive](./superpowers/README.md)

## Safety Boundary

Issue Finder is intentionally conservative.

Allowed:

- Read GitHub issue and repository metadata
- Clone or fetch repositories
- Create or checkout a local Issue Finder branch
- Scan repository files within a limited scope
- Run fixed low-risk probes such as `git status --porcelain`, `git branch --show-current`, `git ls-files`, and package script metadata reads
- Write Issue Finder state under `~/.issue-finder` or `ISSUE_FINDER_HOME`
- After explicit local approvals, start or resume a native execution-agent session, approve outbound A2A artifacts, or post a drafted GitHub comment

Not allowed for the Issue Finder process itself:

- Modify target repository source
- Automatically run target repository validation commands
- Install dependencies
- Commit
- Push
- Create pull requests
- Reset, clean, or delete workspaces

Issue Finder writes suggested validation commands into the handoff package but does not run them automatically.
Validation, build, lint, install, network-heavy, and project-defined script commands are classified as requiring approval or forbidden for downstream agents.
