# Agent Dispatch Control Plane Design

Date: 2026-06-18

Status: Approved for implementation planning

## Summary

Issue Finder should evolve from a local issue preparation CLI into an issue discovery and task dispatch control plane. It remains focused on finding valuable GitHub issues, confirming fit with deterministic evidence and optional LLM review, learning user preferences through explicit feedback, packaging work for execution agents, tracking execution state, and projecting approved status back to GitHub.

Issue Finder should not become a Codex wrapper, a general coding agent, or an A2A-first runtime. A2A is an interoperability boundary for exchanging tasks and artifacts with external agents. Native execution agents keep their own adapter layer so Issue Finder can use their full session and thread capabilities instead of reducing every agent to `run(prompt) -> text`.

The first production adapter is Codex through the OpenAI Codex app-server JSON-RPC protocol. The internal model and user-facing commands stay agent-neutral so future adapters for Claude Code, OpenHands, SWE-agent, and custom HTTP or A2A agents can fit without reworking discovery.

## Goals

- Introduce an Agent Dispatch Runtime that owns task packages, dispatch runs, session links, artifacts, approvals, and event logs.
- Keep Issue Finder's core role as discovery, confirmation, memory/profile learning, dispatch control, and GitHub status projection.
- Preserve native agent capabilities through capability-based adapters.
- Implement Codex first through the Codex app-server JSON-RPC protocol, including thread/session management.
- Add user-visible agent, session, and dispatch commands that are not Codex-specific.
- Move long-term dispatch state to SQLite with artifact files for large payloads.
- Treat GitHub comments as an external projection of local state, not as the source of truth.
- Keep A2A as a gateway protocol for external interoperability rather than the internal database model.

## Non-Goals

- Do not implement a coding agent inside Issue Finder.
- Do not create a one-off `codex_bridge.rs` with Codex-specific workflow logic scattered through discovery or prepare.
- Do not make A2A the internal state model.
- Do not flatten all execution agents into a lowest-common-denominator text prompt API.
- Do not let Issue Finder directly modify target repository source, install dependencies, commit, push, or create pull requests.
- Do not silently update the user profile from LLM output.
- Do not automatically post GitHub comments without user approval.
- Do not remove existing handoff artifacts in the first migration.

## Product Boundary

Issue Finder Agent owns:

- Discovery and ranking.
- Deterministic evidence and value scoring.
- Optional LLM confirmation of issue fit.
- User profile snapshots and explicit memory events.
- Issue task packaging.
- Dispatch approval and routing.
- Local run, session, artifact, and GitHub projection state.

Agent Dispatch Runtime owns:

- `IssueTaskPackage` creation and artifact persistence.
- Dispatch run state.
- Session links to native execution agents.
- Adapter capability checks.
- Event ingestion and result artifact capture.
- Approval requests for dispatch, GitHub writes, A2A sends, and PR-related capabilities.

Native Agent Adapters own:

- Mapping generic capabilities onto native agent APIs.
- Session/thread start, resume, read, rename, archive, and event streaming where supported.
- Converting native outputs into Issue Finder artifacts and events.

A2A Gateway owns:

- Mapping `IssueTaskPackage` artifacts to standard A2A task/message/artifact payloads.
- Accepting external A2A result artifacts and importing them into dispatch state.
- No internal ranking, session management, or Codex-specific behavior.

## Architecture

```text
Issue Finder Agent
  Discovery / ranking / LLM confirmation / memory profile / GitHub projection

Agent Dispatch Runtime
  IssueTaskPackage / dispatch run / session link / artifacts / approvals / event log

Native Agent Adapters
  Codex app-server adapter first
  Future: Claude Code, OpenHands, SWE-agent, custom HTTP/A2A

A2A Gateway
  External interoperability only
  Maps IssueTaskPackage <-> A2A task/message/artifact

Storage
  SQLite for queryable state
  Artifact files for large durable payloads
```

The runtime is intentionally below CLI and JSON tool contract adapters. CLI, JSON tools, and future MCP or A2A front doors should call the same dispatch runtime rather than reimplementing routing, approval, or state transitions.

## Storage Model

Long-term dispatch state should move to SQLite plus artifact files.

SQLite stores queryable state and indexes. Artifact files store large payloads such as task packages, context packs, handoff JSON, transcript extracts, fix results, patches, PR metadata, and GitHub comment bodies.

Core tables:

```text
agent_profiles
  id, kind, display_name, adapter, config_json, enabled

agent_capabilities
  agent_id, capability, status, details_json

issue_tasks
  id, issue_key, repo_full_name, issue_number, title, url
  status, priority, category, created_at, updated_at
  current_package_artifact_id, profile_snapshot_artifact_id

dispatch_runs
  id, issue_task_id, agent_id, status
  requested_by, approval_state, created_at, started_at, completed_at
  selected_session_link_id, result_artifact_id, failure_reason

agent_session_links
  id, agent_id, native_session_id, issue_task_id
  display_name, goal, status, metadata_json
  created_at, last_seen_at, archived_at

agent_events
  id, run_id, session_link_id, event_type, native_event_id
  payload_json, created_at

agent_artifacts
  id, issue_task_id, run_id, kind, path, content_type
  sha256, created_at, metadata_json

github_interactions
  id, issue_task_id, interaction_type, github_comment_id
  body_artifact_id, status, created_at, posted_at, error

approval_requests
  id, run_id, approval_type, status, prompt, details_json
  created_at, resolved_at

memory_events
  id, issue_task_id, event_type, source, payload_json, created_at
```

Existing inbox and recommendation files remain readable. New dispatch state should not keep extending JSONL files as the primary source of truth.

## Issue Task Package

`handoff.json` becomes one artifact inside a broader portable unit of work.

```text
IssueTaskPackage
  issue
  evidence
  llm_confirmation
  user_profile_snapshot
  workspace_policy
  context_pack
  validation_hints
  expected_outputs
  callback_policy
```

The package should be serializable as `issue_task_package.json`, with optional companion artifact files for large context sections. It should contain enough information for a native adapter or A2A recipient to understand the task without re-running discovery.

`callback_policy` describes what Issue Finder expects back:

- `fix_result.json` with summary, changed files, validation, residual risks, and suggested GitHub reply.
- Optional patch artifact.
- Optional PR link artifact.
- Optional native session link artifact.

## State Machines

Issue task states:

```text
discovered
  -> llm_confirmed
  -> user_approved
  -> dispatched
  -> in_progress
  -> fix_ready
  -> github_posted
  -> done
```

Dispatch run states:

```text
proposed
  -> approved
  -> queued
  -> starting
  -> running
  -> needs_user
  -> completed | failed | canceled
```

Agent session link states:

```text
linked
  -> active
  -> idle
  -> archived | failed
```

GitHub interactions have independent states because GitHub write state is an external projection:

```text
draft
  -> approved
  -> posted | failed
  -> retried
```

## Capability-Based Agent Adapters

Every adapter declares supported capabilities. CLI and tools check capabilities before offering or attempting an operation.

```text
AgentCapabilities
  start_session
  resume_session
  list_sessions
  search_sessions
  rename_session
  fork_session
  archive_session
  stream_events
  read_transcript
  set_goal
  set_metadata
  interrupt_run
  review_mode
  open_pr
```

Capabilities are not only booleans. `agent_capabilities.details_json` may describe limits, protocol version, experimental status, required local binaries, or unsupported fields.

Issue Finder should not emulate unsupported native capabilities by guessing. If an adapter cannot rename a session, `sessions rename` returns a capability error.

## User-Facing CLI

User-facing commands should use generic agent/session/dispatch terminology.

```bash
issue-finder agents list
issue-finder agents capabilities codex

issue-finder sessions list --agent codex
issue-finder sessions search --issue owner/repo#123
issue-finder sessions read <link-id>
issue-finder sessions rename <link-id> --name "issue-finder: owner/repo#123 ..."
issue-finder sessions archive <link-id>

issue-finder dispatch owner/repo#123 --agent codex --new-session
issue-finder dispatch owner/repo#123 --agent codex --session <native-or-link-id>
issue-finder dispatch status <run-id>
issue-finder dispatch events <run-id>
issue-finder dispatch artifacts <run-id>
```

`--session` resumes or continues an existing native session. `--new-session` creates a new native session. If both are omitted, the runtime may propose a session based on existing `agent_session_links`, but dispatch still requires approval.

Session names should be deterministic by default:

```text
issue-finder: owner/repo#123 - short title
```

## JSON Tool Contract

The existing JSON tool contract should expand with agent-neutral tools:

```text
issue-finder.agents_list
issue-finder.agent_capabilities
issue-finder.sessions_list
issue-finder.sessions_search
issue-finder.sessions_read
issue-finder.sessions_rename
issue-finder.dispatch
issue-finder.dispatch_status
issue-finder.dispatch_events
issue-finder.dispatch_artifacts
```

Tool outputs must follow the existing structured output pattern:

- Invalid arguments and system errors use `success=false`.
- Business blocks such as missing capability or pending approval are structured statuses, not panics.
- Large transcripts and artifacts should be deferred behind artifact ids or section reads.

## Codex App-Server Adapter

The first native adapter is `codex_app_server`.

It should use the local Codex app-server JSON-RPC protocol instead of `codex exec` or `codex mcp-server` because app-server exposes full thread, turn, metadata, and review operations.

Startup behavior:

- Detect `codex` availability.
- Start or connect to the app-server daemon using `codex app-server daemon start` or `codex app-server proxy`.
- Read app-server version when available.
- Probe supported methods and cache capabilities.
- Record capability details in `agent_capabilities`.

Capability mapping:

```text
start_session       -> thread/start
resume_session      -> thread/resume
fork_session        -> thread/fork
rename_session      -> thread/name/set
list_sessions       -> thread/list
search_sessions     -> thread/search
read_transcript     -> thread/read, thread/turns/list, thread/turns/items/list
set_goal            -> thread/goal/set
set_metadata        -> thread/metadata/update
archive_session     -> thread/archive
interrupt_run       -> turn/interrupt
review_mode         -> review/start
dispatch prompt     -> turn/start
```

The adapter stores Codex thread id as `agent_session_links.native_session_id`. It stores turn ids, streamed events, tool calls, plan updates, file changes, and final messages as `agent_events` or artifact files depending on size.

Dispatch prompt template:

```text
You are receiving an Issue Finder task package.
Goal: locate, reproduce if practical, and fix the GitHub issue.
Read the package artifacts first.
Respect workspace_policy.
Return a FixResult artifact with summary, files changed, validation run,
residual risks, and suggested GitHub reply.
```

The adapter must not bypass Codex's native approval and sandbox behavior. If Codex asks for user approval, Issue Finder records a `needs_user` run state and links to the native session.

## A2A Gateway

A2A is an external gateway.

Outbound mapping:

```text
IssueTaskPackage -> A2A Task: fix_github_issue
input artifact: issue_task_package.json
input artifact: context_pack.zip or local artifact refs
expected artifact: fix_result.json
optional artifact: patch / pr_link / session_link
```

Inbound mapping:

```text
A2A result artifact -> agent_artifacts
A2A task status -> dispatch_runs.status
A2A messages/events -> agent_events
```

A2A gateway does not manage Codex threads, set Codex metadata, or read Codex transcripts. Those remain adapter responsibilities.

Outbound A2A sends require user approval because they may expose local issue package context outside the machine or current app boundary.

## GitHub Projection

GitHub write support should be explicit and approval-gated.

Interaction types:

```text
tracking_comment
  "I am tracking this issue and preparing a fix attempt."
  Default behavior: generate draft, require approval to post.

progress_comment
  Intermediate status. Disabled by default.

final_comment
  Summary of fix result, validation, PR or patch link, or reason no fix was produced.
  Default behavior: generate draft, require approval to post.
```

GitHub write client requirements:

- Use existing GitHub client boundaries or a clearly separated write client.
- Use mockable HTTP calls in tests.
- Record comment ids after successful post.
- Record failures and support retry.
- Never use a posted comment as the local source of truth.

The recommendation system must recognize Issue Finder's own tracking comments so it does not misclassify them as unrelated competition or claim signals.

## Memory and Profile Learning

Memory is event-sourced and explainable. LLMs may propose changes, but they cannot silently mutate profile config.

Memory event types:

```text
positive_signal
  User approved dispatch, completed fix, or marked issue high quality.

negative_signal
  User rejected issue, canceled run, or marked recommendation noisy or unsuitable.

profile_adjustment_candidate
  Candidate profile change derived from long-term feedback.

agent_performance_signal
  Agent success, failure, runtime, or user intervention for a task class.
```

Profile changes should remain reviewable by a user or main agent. Future profile tools can aggregate `memory_events` into proposed `[profile]` edits, but the first dispatch runtime should only record evidence and candidates.

## Safety and Approval Policy

Automatically allowed:

- Reading Issue Finder artifacts.
- Creating `IssueTaskPackage` artifacts.
- Querying local dispatch state.
- Listing sessions when adapter capability exists.
- Generating GitHub comment drafts.

Requires approval:

- Dispatching to an execution agent.
- Continuing or resuming an existing native session for a new issue task.
- Posting GitHub comments.
- Sending A2A tasks or artifacts outside Issue Finder.
- Using any `open_pr` capability.
- Renaming or archiving native sessions when the action is not a direct consequence of an approved dispatch.

Forbidden for Issue Finder itself:

- Modify target repository source.
- Install dependencies.
- Commit.
- Push.
- Create pull requests.
- Reset, clean, or delete target workspaces.

Execution agents may perform their own actions inside native sessions subject to their native approval, sandbox, and user control. Issue Finder records what happened; it does not take over execution.

## Migration

The migration should be additive.

- Keep existing `scout`, `assess`, `prepare`, `handoff`, `daily`, inbox, and report behavior while dispatch runtime is introduced.
- Add SQLite store under `IssueFinderPaths`.
- Add a migration/import path that can turn an existing prepared handoff into an `IssueTaskPackage` artifact and `issue_tasks` row.
- Keep generated handoff/context pack artifacts as package artifacts.
- Do not delete or rewrite old inbox items during the first migration.
- Documentation should be updated so handoff is described as a durable artifact, not the final product boundary.

## Testing

Storage tests:

- Use temporary `ISSUE_FINDER_HOME` or explicit temp `IssueFinderPaths`.
- Verify schema creation, inserts, updates, idempotent imports, and query behavior.
- Verify artifact sha256 and path persistence.

Codex adapter tests:

- Use a fake app-server JSON-RPC server or process stub.
- Cover capability detection.
- Cover new session dispatch.
- Cover existing session dispatch.
- Cover rename, archive, read transcript, event capture, and failure mapping.
- Do not require real Codex in automated tests.

GitHub projection tests:

- Use local mock HTTP server.
- Cover draft generation, approved post, failed post, retry, and comment id persistence.
- Cover self-authored tracking comment handling so recommendation competition signals do not regress.

Dispatch workflow tests:

- Cover task package creation.
- Cover approval-required dispatch.
- Cover run state transitions.
- Cover artifact persistence.
- Cover adapter capability errors.
- Cover A2A outbound approval and artifact mapping with local fixtures.

Recommendation and memory tests:

- If profile, feedback, freshness, ranking, or quality policy changes, update `tests/fixtures/recommendation_eval/` or document why no fixture is needed.
- Memory events should be deterministic and independent of real GitHub, real Codex, or real LLM services.

## Documentation Updates

When implementing this design, update:

- `README.md` and `README.zh-CN.md` to describe Issue Finder as discovery and dispatch control plane.
- `docs/usage.md` with agent/session/dispatch commands.
- `docs/issue-finder-rust-design.md` so old "no agent adapters" statements do not conflict with this design.
- `docs/agent-safe-preparation-runtime.md` to clarify that handoff/context artifacts remain part of the task package.
- Tool contract docs when new JSON tools are added.

## Acceptance Criteria

- Issue Finder has a SQLite-backed dispatch store and artifact persistence layer.
- Agent profiles and capabilities can be listed without naming Codex in the generic CLI surface.
- Codex app-server adapter can create, resume, rename, read, and archive Codex sessions through native thread APIs.
- Dispatch can create an `IssueTaskPackage`, require approval, start or resume a native Codex session, send the first turn, and persist events and artifacts.
- GitHub tracking and final comments are draft-first and approval-gated.
- A2A gateway can map a task package to an outbound task artifact without becoming the internal state model.
- Existing discovery, assessment, prepare gate, handoff generation, and recommendation eval behavior remain intact unless explicitly changed by a later implementation plan.
