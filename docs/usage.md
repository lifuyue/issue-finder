# Issue Finder Usage Guide

Issue Finder serves Codex through discovery and assessment tools. Codex performs
the authorized contribution using its normal tools. Separate legacy terminal,
handoff, and dispatch commands remain outside this session contract.

## Current-session tools

Use [`skills/issue-finder-cli/SKILL.md`](../skills/issue-finder-cli/SKILL.md)
for call conditions, parameters, evidence interpretation, and refresh rules.
The default tools and MCP profiles are `session`; explicit `--profile session`
is equivalent. The installed catalog defines the schemas:

```bash
issue-finder tools list
```

Require `sessionContractVersion: 2` with only `issue-finder.scout` and
`issue-finder.assess`. Cloud environment configuration owns CLI installation,
dependencies, PATH, and supported credentials. Configuration, authentication,
and network failures return directly from the business call. Missing optional
configuration is accepted; no independent readiness call is required.
Credentials resolve from `GITHUB_TOKEN`, optional `[github].token`, then captured
host `gh` authentication. Tokens are not printed or saved by that fallback.

| Tool | Role | Arguments |
| --- | --- | --- |
| `issue-finder.scout` | Discover, filter, and rank candidates | `repo`, `limit`, `search`, `profile`, `refresh`, `includeFiltered`, `recordExposure` |
| `issue-finder.assess` | Read issue, discussion, competition, and repository evidence | `issue` or `url`, `commentsPage`, `commentsPerPage`, `profile`, `refresh`, `recordRead` |

Start broad interest-based discovery with the curated feed:

```bash
issue-finder tools call issue-finder.scout --arguments '{"limit":8,"profile":{"techStack":["Rust"],"keywords":["cli"]}}'
```

For a bounded GitHub search:

```bash
issue-finder tools call issue-finder.scout --arguments '{"repo":"owner/repo","limit":8,"search":{"query":"label:bug","sort":"updated","order":"desc","page":1,"perPage":30,"maxPages":2,"apiBudget":120},"profile":{"techStack":["Rust"],"keywords":["cli"]}}'
```

Omit `repo` for discovery across repositories and `search` for the curated feed.
Global searches need relevant terms or qualifiers. `repo` is a hard boundary,
including with conflicting query qualifiers. `search.sort` accepts `updated`,
`created`, `comments`, or `best_match`; `order` accepts `asc` or `desc`. GitHub
sorting selects retrieved issues; Issue Finder ranks them separately. Pagination
and API budget bound the work. Inspect `structured_content.candidates`,
`diagnostics.search`, `apiBudget`, filter reasons, and warnings before expanding.

Carry scout's resolved `profile` into later assessments and discussion pages.
Overrides apply to one call; omission restores configured defaults. Inherited
preferences are not necessarily the user's stated interests.

An explicit issue skips discovery:

```bash
issue-finder tools call issue-finder.assess --arguments '{"issue":"owner/repo#123","commentsPage":1,"commentsPerPage":30,"profile":{"techStack":["Rust"],"keywords":["cli"]}}'
```

Read `structured_content.issue.body`, relevant comments, `assessment`,
`competition`, repository/activity evidence, and warnings. Follow
`issue.nextCommentsPage` when needed; inspect `commentsTruncated` and
`totalComments`. Recommendation scores and low repository popularity inform
selection rather than impose a repair authorization gate. Partial evidence,
truncation, and a clear competition score do not prove that no competing PR
exists. Check relevant linked/unlinked PRs and current code when acting on that
assumption.

If no usable recommendation remains, `diagnostics.search.diagnosticCandidates`
can provide bounded investigation leads with exclusion reasons. They are not a
vetted shortlist. Parse `success`, `status`, warnings, and error details on every
call: `no_candidates` does not prove there are no matching GitHub issues;
`issue_unavailable` identifies closed issues or PR references; `partial` discloses
missing evidence. Locked or assigned open issues remain assessable and return
`issueWarnings` for Codex to interpret.

Use `refresh: true` when cached evidence may be stale, on resuming an old
investigation, or before acting on changed issue ownership or competition.
Assess retrieves the selected issue/discussion page at call time; refresh also
updates cached assessment evidence. Fetch further pages only when relevant.
Honor rate-limit retry information and coordinate API budgets across parallel work.

Shown/read events are recorded automatically unless `recordExposure: false` or
`recordRead: false` opts out. Historical dismissed/done/prepared states are
ignored for Codex rankings. There is no manual cross-chat ignore/restore entry
and no CLI workspace task, validation result, or PR completion record.

Codex owns choice, workspace preparation, reproduction, repair, validation,
review, and authorized PR delivery. Recommendation-only requests end with the
shortlist. Read access does not establish fork/push/upstream PR permission;
local checks do not establish PR creation, CI success, or merge. The removed
lifecycle tools are not a hidden workflow within `scout` or `assess`.

The sections below document separate legacy terminal and handoff/dispatch paths.
Their control catalog requires `issue-finder tools --profile control list` and
its approval model applies to dispatch. See [Agent Loop Architecture](./agent-loop-target-architecture.md)
for legacy module ownership.

## Legacy handoff and dispatch workflow

```text
Discover good first issues
  -> Rank with local heuristics
  -> Prepare repository workspace
  -> Run fixed low-risk preparation probes
  -> Generate handoff, policy, probe, event, and context artifacts
  -> Store the task in the local inbox
  -> Import handoff for issue review when dispatch or projection is requested
  -> Freeze an immutable ContextSnapshot
  -> Create TaskPackage only after review approval
  -> Launch one persistent supervisor for the approved run
  -> Track Codex threads, turns, items, approvals, pending requests, and candidate results
  -> Deterministically evaluate results and retry within budget
  -> Commit one terminal outcome and project downstream state
  -> Project local candidate/task board state for queries
  -> Generate a daily report
```

`handoff.json` remains the canonical prepared handoff artifact. In the dispatch control plane, prepared handoffs and task packages are durable artifacts tracked alongside runs, native Codex threads, approvals, events, and result artifacts. `handoff.md` is the human-readable summary. `agent-policy.json`, `probe.json`, `prepare-events.jsonl`, `codex.md`, and `context/*.md` give downstream coding agents a safer starting point. See [Agent-Safe Preparation Runtime](./agent-safe-preparation-runtime.md) for the full artifact model.

## Requirements

- An installed Issue Finder binary and Git
- GitHub credentials with read access for discovery
- Rust 1.89+ and Cargo only when installing/building from source

Optional:

- GitHub CLI (`gh`), useful for reusing an existing GitHub token
- GitHub issue write permission, needed only if you explicitly approve and post GitHub comments through dispatch projection commands
- OpenAI-compatible API key, used only when optional LLM summaries are enabled
- Codex CLI with `app-server`, required only for native Codex thread dispatch. Set `ISSUE_FINDER_CODEX_BIN` to override discovery; otherwise Issue Finder validates the PATH binary and, on macOS, can discover the Codex binary bundled with ChatGPT.app. The default daemon transport additionally requires the installer-managed standalone Codex under `CODEX_HOME/packages/standalone/current/codex`; the bundled binary alone is sufficient only for the explicitly selected stdio fallback.

The CLI verifies HTTPS certificates against bundled public roots and the system's
trusted CA store. For an HTTPS proxy or a private endpoint, install its CA through
the operating system's trust settings. On Linux, `SSL_CERT_FILE` and
`SSL_CERT_DIR` can select a custom CA bundle or certificate directory. Certificate
verification remains enabled for GitHub and optional LLM requests.

## Installation

Install the published crate:

```bash
cargo install issue-finder --locked
issue-finder --help
```

Prebuilt platform archives and SHA-256 checksums are available from the
[official stable release channel](https://github.com/lifuyue/issue-finder/releases/latest).
For the CLI skill, verify `sessionContractVersion: 2` in the session catalog after
installation. If the published stable release does not yet support it, explicitly
install this checkout with `cargo install --path . --locked`; the skill does not
silently fall back to source commands.

For CLI development from this checkout, use Cargo directly:

```bash
cargo run -- --help
```

## GitHub Token

Issue Finder uses the GitHub REST API to discover issues and read repository metadata. Discovery, assessment, preparation, and local dispatch state only need read access.

`GITHUB_TOKEN` takes precedence over `[github].token` in configuration. Session
tools additionally reuse the host's `gh` authentication automatically, without
printing or saving its secret. Terminal and control-profile operations use the
environment/configuration sources. Provide their token through your environment
or secret manager, or enter one during optional `issue-finder init`.
Inherited environment tokens are not displayed or copied into saved defaults.

GitHub write permission is optional. It is used only by `issue-finder dispatch github post <interaction-id>` after a local interaction policy decision creates a draft and a human approves the `github_post` approval request.

## Common Commands

Optionally initialize local configuration and directories for terminal workflows:

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

Check the separate control-profile JSON tools for a handoff/dispatch integration:

```bash
issue-finder tools --profile control list
issue-finder tools --profile control call issue-finder.status --arguments '{}'
```

Issue Finder does not start an A2A listener by default. `dispatch a2a` is an explicitly invoked external artifact gateway that maps approved packages and results onto the same dispatch artifacts and lifecycle; it is not a second agent runtime or database.

Discover and rank candidate issues:

```bash
issue-finder scout --limit 10
issue-finder scout --repo owner/repo --limit 10
issue-finder scout --refresh
issue-finder scout --stats-json
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
issue-finder dispatch package import-handoff <inbox-id>
issue-finder dispatch review list
issue-finder dispatch review show <approval-request-id>
issue-finder dispatch review approve <approval-request-id>
issue-finder dispatch review reject <approval-request-id> --reason "..."
issue-finder dispatch owner/repo#123 --agent codex
issue-finder dispatch owner/repo#123 --agent codex --new-session
issue-finder dispatch owner/repo#123 --agent codex --session <codex-thread-id>
issue-finder dispatch approve <run-id>
issue-finder dispatch execute <run-id>
issue-finder dispatch sync <run-id>
issue-finder dispatch a2a export owner/repo#123
issue-finder dispatch a2a approve <approval-request-id>
issue-finder dispatch a2a reject <approval-request-id>
issue-finder dispatch a2a import-result <run-id> --path ./candidate_result.json
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

Direct dispatch creates a new Codex session proposal when `--session` is omitted. `--new-session`
is the explicit form of that same start-session request and cannot be combined with `--session`.
`dispatch execute` starts a detached supervisor; `dispatch sync` only reads the supervisor's
durable projection and never opens a competing app-server connection.

JSON and MCP control callers use `issue-finder.dispatch`, `issue-finder.dispatch_sync`,
`issue-finder.dispatch_pending_requests`, `issue-finder.dispatch_respond`,
`issue-finder.dispatch_steer`, and `issue-finder.dispatch_interrupt`. The supervisor exposes a
separate run-scoped worker MCP profile containing only `issue-finder.read_context` and
`issue-finder.submit_result`.

Manage local inbox items:

```bash
issue-finder inbox
issue-finder inbox --json
issue-finder inbox archive <inbox-id>
issue-finder inbox done <inbox-id>
```

Record or inspect legacy recommendation feedback (Codex rankings ignore manual
dismissed/done/prepared state):

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
issue-finder eval codex-runtime --workspace <absolute-path> --timeout-seconds 120
```

`eval codex-runtime` is a real process-level acceptance probe, not a mock. Success
requires an app-server handshake plus a completed authenticated model response containing
the per-run marker in the persisted transcript. Runtime absence, failed/interrupted turns,
and timeouts are emitted as structured `capability_unavailable` outcomes so an external
harness can grade unavailable infrastructure separately from product correctness.

The external engineering harness uses Inspect AI and Harbor as two runtime adapters over
one task, evidence, ATIF, and independent-verifier contract. The complete architecture,
outcome semantics, hard gates, and canonical 50-task catalog are defined in
[Evaluation Engineering](./evaluation-engineering.md).

## Command Reference

| Command | Purpose |
| --- | --- |
| `issue-finder init` | Create local config and Issue Finder state directories |
| `issue-finder profile bootstrap --json` | Scan supported local Agent indexes and project manifests, then print a profile bootstrap report |
| `issue-finder doctor` | Check Git, GitHub auth, config, directory permissions, platform, and optional LLM status |
| `issue-finder tools --profile session list` | Print current-session tools and `sessionContractVersion` |
| `issue-finder tools list` | Print the default Codex discovery/assessment catalog |
| `issue-finder tools --profile control list` | Print the separate legacy control catalog |
| `issue-finder tools --profile control call issue-finder.status --arguments '{}'` | Return JSON config, token source, and GitHub auth diagnostics without printing tokens |
| `issue-finder scout --limit 10` | Discover and rank good-first-issue candidates |
| `issue-finder scout --repo owner/repo --limit 10` | Discover and rank candidates strictly within one repository |
| `issue-finder scout --refresh` | Ignore the local GitHub issue cache and request fresh data |
| `issue-finder scout --json` | Print ranked candidates as JSON |
| `issue-finder scout --stats-json` | Print ranked candidates plus discovery, filter, cache, and API budget stats as JSON |
| `issue-finder assess owner/repo#123` | Assess one issue without preparing workspace or handoff state |
| `issue-finder assess --url <url>` | Assess one issue from a GitHub issue URL |
| `issue-finder prepare owner/repo#123` | Prepare one issue and write it to the inbox |
| `issue-finder prepare --url <url>` | Prepare one issue from a GitHub issue URL |
| `issue-finder handoff <id>` | Display an existing handoff |
| `issue-finder handoff <id> --print` | Print human-readable `handoff.md` |
| `issue-finder handoff <id> --json` | Print canonical `handoff.json` |
| `issue-finder agents list` | List local execution agent profiles |
| `issue-finder agents capabilities codex` | List one agent's declared native capabilities; wired Codex app-server session capabilities are experimental, while unwired capabilities such as `stream_events`, `interrupt_run`, `review_mode`, and `open_pr` are reported as unsupported |
| `issue-finder agents probe codex` | Start the discovered Codex app-server, complete its initialize handshake, run `thread/list`, and cache the result; method mappings alone never count as support |
| `issue-finder dispatch package import-handoff <id>` | Import an existing inbox handoff as an `issue_review` candidate and create an approval request |
| `issue-finder dispatch review list` | List pending and resolved issue review requests |
| `issue-finder dispatch review show <approval-request-id>` | Show one issue review request, including imported handoff/package evidence |
| `issue-finder dispatch review approve <approval-request-id>` | Approve one issue review and create the immutable `TaskPackage` and `ContextSnapshot` artifacts |
| `issue-finder dispatch review reject <approval-request-id>` | Reject one issue review without dismissing the recommendation |
| `issue-finder dispatch owner/repo#123 --agent codex` | Create a pending dispatch approval for a new native session by default; returns `pending_issue_review` first if the package has not been review-approved |
| `issue-finder dispatch owner/repo#123 --agent codex --new-session` | Explicit form of the default new-session dispatch proposal; returns `pending_issue_review` first if the package has not been review-approved |
| `issue-finder dispatch owner/repo#123 --agent codex --session <codex-thread-id>` | Create a pending approval to continue one explicitly selected native Codex thread; returns `pending_issue_review` first if needed |
| `issue-finder dispatch approve <run-id>` | Resolve a pending dispatch approval and move the run to `approved` |
| `issue-finder dispatch reject <run-id>` | Reject a pending dispatch approval and cancel the run |
| `issue-finder dispatch execute <run-id>` | Start the run's persistent Codex supervisor after local approval |
| `issue-finder dispatch sync <run-id>` | Read the durable run projection without opening another Codex connection |
| `issue-finder dispatch a2a export owner/repo#123` | Create a local A2A task artifact from the approved `TaskPackage` and an `a2a_send` approval request without network I/O; returns `pending_issue_review` first if needed |
| `issue-finder dispatch a2a approve <approval-request-id>` | Approve an outbound A2A task artifact for external use |
| `issue-finder dispatch a2a reject <approval-request-id>` | Reject an outbound A2A task artifact |
| `issue-finder dispatch a2a import-result <run-id> --path <file>` | Submit a local `CandidateResult` through the same deterministic evaluator used by the Codex worker |
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
| `issue-finder feedback dismiss <issue>` | Hide an issue from legacy recommendation feed results |
| `issue-finder feedback restore <issue>` | Restore a done or dismissed issue to the legacy recommendation feed |
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

The Codex session contract uses caches and automatic shown/read recommendation
events; it does not create workspace task files or persist implementation/check
results. Existing legacy `sessions/` data and `.issue-finder-cli-task.json` files
are unused by this contract. Historical dismissed/done/prepared feedback is
ignored in Codex rankings rather than silently hiding candidates after removal
of the manual restore entry. The legacy control/terminal paths retain their own
state semantics.

`state.sqlite3` stores contribution memory tables. `dispatch/dispatch.sqlite3` is the unified source for dispatch, native threads/turns/items/events/outbox, approvals, A2A mappings, and GitHub projection; dispatch artifacts live under `dispatch/artifacts/`.

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

When dispatch state is used, `handoff.json` is imported as an issue-review candidate first. Review
approval freezes all referenced context into a content-addressed `ContextSnapshot` and writes a
`TaskPackage`. The package is the execution contract: goal, constraints, success criteria,
validation commands, interaction policy, runtime budget, exact snapshot, and `CandidateResult`
schema. Commands can auto-import a ready inbox handoff, but they return `pending_issue_review`
until the review is approved.

Codex communication uses one bidirectional stdio app-server connection owned by the detached run
supervisor. The supervisor owns discovery, thread/turn lifecycle, pending request routing, event
persistence, control/result outboxes, retries, and restart recovery. `dispatch/dispatch.sqlite3`
stores the same stream as durable threads, turns, items, events, pending requests, artifacts,
evaluator reports, and outcomes. `dispatch sync` reads that projection; it never creates another
connection or treats `turn/completed` as success. A2A is an explicit gateway into the same package
and candidate-result evaluator, not another agent loop or store.

An external OpenAI-compatible provider can be selected without writing its secret into Issue Finder state. Set `ISSUE_FINDER_CODEX_MODEL`, `ISSUE_FINDER_CODEX_MODEL_PROVIDER`, `ISSUE_FINDER_CODEX_PROVIDER_NAME`, `ISSUE_FINDER_CODEX_BASE_URL`, `ISSUE_FINDER_CODEX_WIRE_API`, and `ISSUE_FINDER_CODEX_REASONING_EFFORT`; set `ISSUE_FINDER_CODEX_API_KEY_ENV` to the *name* of the inherited environment variable containing the credential. Provider and reasoning overrides are passed to every daemon or stdio app-server process, so a resumed thread uses the same runtime configuration.

`agents probe codex --refresh` probes the same transport used by dispatch. A successful handshake leaves method mappings unverified until a conformance action runs; CLI help or schema presence alone is not support evidence. Arbitrary remote thread metadata is not an app-server capability, so Issue Finder keeps run linkage in its own store. Runtime failures never rewrite permanent product-policy decisions such as `open_pr`, `review_mode`, or `stream_events`; those remain unsupported.

The candidate task board is a derived library-level read model over recommendation events, inbox items, and dispatch state. It is a query surface, not a persisted source of truth. Dispatch terminal outcomes remain visible as terminal board status even if an inbox item was marked done or archived; archive and dismiss feedback only affect display state. Reactivation is also projected locally and does not change recommendation feed score or memory ranking adjustments.

Runtime topic docs:

- [Sandbox & approvals](./sandbox.md)
- [Execution policy](./execpolicy.md)
- [Safe probes](./safe-probes.md)
- [Skills and context pack](./skills.md)
- [Superpowers design archive](./superpowers/README.md)

## Execution boundaries

The Codex session profile reads GitHub, ranks candidates, reports evidence, and
can record automatic shown/read events. Configuration and credential errors are
returned directly by `scout` or `assess`. These tools do not prepare workspaces,
run target validation commands, record completion, or publish to GitHub. Codex
owns those operations using normal tools under the user's scope, host permissions,
and applicable repository instructions. Cloud environment configuration supplies
installation, dependencies, PATH, and supported authentication.

The following boundaries describe handoff preparation and the separate dispatch
control plane.

Allowed:

- Read GitHub issue and repository metadata
- Clone or fetch repositories
- Create or checkout a local Issue Finder branch
- Scan repository files within a limited scope
- Run fixed low-risk probes such as `git status --porcelain`, `git branch --show-current`, `git ls-files`, and package script metadata reads
- Write Issue Finder state under `~/.issue-finder` or `ISSUE_FINDER_HOME`
- After explicit local approvals, start or resume a native execution-agent session, approve outbound A2A artifacts, or post a drafted GitHub comment

Not performed by handoff preparation:

- Modify target repository source
- Automatically run target repository validation commands
- Install dependencies
- Commit
- Push
- Create pull requests
- Reset, clean, or delete workspaces

Handoff preparation writes suggested validation commands into the handoff
package without running them. Its downstream policy classifies validation,
build, lint, install, network-heavy, and project-defined scripts for dispatch
approval. This legacy policy is outside the Codex discovery/assessment contract.
# External evaluation contract

Evaluation harnesses should negotiate capabilities before running:

```bash
issue-finder eval contract --json
```

The response is the product-side source of truth for runtime and artifact versions,
normalized termination vocabulary, recovery boundaries, benchmark families, and safety
invariants. Harnesses should reject unsupported versions instead of probing deprecated
commands.
