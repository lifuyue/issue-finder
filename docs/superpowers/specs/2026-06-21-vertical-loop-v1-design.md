# Vertical Loop V1 Design

Date: 2026-06-21

Status: Implemented in `fuyue/vertical-loop-v1`

## Summary

Vertical Loop V1 turns a prepared Issue Finder handoff into a review-gated dispatch package. Prepared handoffs now carry typed LLM confirmation evidence, but a human review approval is required before Issue Finder writes an `IssueTaskPackage` artifact or dispatches work to an execution agent.

The loop is intentionally vertical and narrow:

```text
prepare
  -> typed LLM confirmation in handoff
  -> import handoff as issue review candidate
  -> human issue review approval
  -> IssueTaskPackage v3
  -> dispatch proposal approval
  -> agent execution / A2A / GitHub projection
  -> local outcome artifacts and memory signals
```

## Goals

- Make the existing `llm_confirmed -> user_approved -> dispatched` issue task states meaningful.
- Keep LLM output non-authoritative and reviewable.
- Require human approval before package creation.
- Keep dispatch approval separate from issue review approval.
- Define the package v3 outcome contract consumed by execution agents and future recommendation feedback.
- Emit dispatch/memory signals for issue review decisions without changing recommendation ranking in this branch.

## Non-Goals

- Do not implement a general dispatch event bus.
- Do not rewrite recommendation ranking or feedback consumption.
- Do not implement Codex adapter internals.
- Do not duplicate prepare gate rules outside `src/prepare_gate.rs`.
- Do not map issue review rejection directly to recommendation `Dismissed`.

## Owner Boundaries

- `src/llm_review.rs` owns typed LLM confirmation DTOs and parsing.
- `src/handoff.rs` and `src/context_pack.rs` own handoff/context rendering compatibility.
- `src/dispatch/task_package.rs` owns the typed package v3 contract and builder.
- `src/dispatch/packaging.rs` owns handoff import, issue review approvals, and package creation.
- `src/dispatch/runtime.rs`, `src/dispatch/tools.rs`, and `src/dispatch/cli.rs` expose review operations through thin adapters.
- `src/dispatch/memory.rs` owns local memory signals emitted by dispatch decisions.
- `src/recommendation/*` remains the owner for whether outcome signals affect ranking.

## LLM Confirmation

New prepare output includes `llm_confirmation`:

```json
{
  "status": "success",
  "decision": "confirm",
  "confidence": 0.82,
  "summary": "Good fit for a small Rust CLI fix",
  "fit_notes": ["small scope"],
  "risk_flags": ["needs validation"],
  "missing_context": [],
  "source_refs_used": ["issue.body"],
  "agent_brief": "Start in src/lib.rs",
  "warnings": []
}
```

LLM confirmation cannot change deterministic scores, recommendation category, prepare gate decisions, or dispatch policy. Disabled, empty, malformed, or failing LLM responses produce non-blocking `disabled` or `failed` confirmation statuses.

The legacy free-form `llm_review` field remains readable/renderable for old handoffs, but package v3 does not use it as confirmation evidence.

## Issue Review

Importing a ready inbox handoff creates an `approval_requests` row with `approval_type = "issue_review"` and does not create a package artifact.

Review approval:

- Resolves the issue review approval as approved.
- Writes `IssueTaskPackage.version = 3`.
- Updates the issue task to `user_approved`.
- Records a positive `issue_review` memory signal.

Review rejection:

- Resolves the issue review approval as rejected.
- Does not write a package artifact.
- Does not dismiss the recommendation.
- Records a negative `issue_review` memory signal.
- Blocks dispatch/A2A/GitHub projection for that task.

## IssueTaskPackage V3

Package v3 is the current handoff contract for execution agents. It replaces the earlier loose package shape with typed sections:

```text
source
human_review
memory_context
reproduction_contract
success_criteria
change_budget
environment_contract
interaction_policy
session_context
outcome_contract
```

The package includes typed `llm_confirmation`, value evidence, profile snapshot, workspace policy, context pack metadata, validation hints, and callback policy. Existing local v2 artifacts are not migrated in place; users can re-import and approve a ready handoff to create a v3 artifact.

The reproduction contract does not infer steps from arbitrary issue text. It states the agent's obligations: read issue/context evidence, attempt reproduction when practical, record commands and observations, and report blockers instead of fabricating evidence.

The outcome contract requires `fix_result.json` with:

```text
status
summary
changedFiles
reproduction
successCriteria
validation
residualRisks
failureReason
sessionContext
suggestedGitHubReply
```

Optional artifacts are `patch`, `pr_link`, `session_link`, and `validation_log`.

## Tool And CLI Contract

CLI:

```bash
issue-finder dispatch package import-handoff <inbox-id>
issue-finder dispatch review list
issue-finder dispatch review show <approval-request-id>
issue-finder dispatch review approve <approval-request-id>
issue-finder dispatch review reject <approval-request-id> --reason "..."
```

JSON tools:

```text
issue-finder.dispatch_review_list
issue-finder.dispatch_review_show
issue-finder.dispatch_review_approve
issue-finder.dispatch_review_reject
```

Direct dispatch and projection tools may auto-import a matching ready handoff, but they return `pending_issue_review` until review approval creates a package. They never auto-approve issue review or dispatch.

## Error Handling

`pending_issue_review` is a structured business block:

```json
{
  "status": "pending_issue_review",
  "blocked": true,
  "issueKey": "owner/repo#123",
  "approvalRequestId": "approval-1",
  "reviewRequired": true
}
```

Rejected review returns `issue_review_rejected` for JSON tools. Missing package source still returns `missing_task_package`.
