# IssueTaskPackage V3 Design

Date: 2026-06-21

Status: Implemented in `fuyue/issue-task-package-v3`

## Summary

`IssueTaskPackage` v3 is the current dispatch handoff contract for execution agents. It replaces the earlier loose v2 package shape with typed sections that tell an agent what to reproduce, how to judge success, how much change is acceptable, which environment and interaction boundaries apply, how to resume, and what result artifact to return.

Issue Finder still only prepares local workspaces and artifacts. It does not modify target repository source, install dependencies, commit, push, create pull requests, or post GitHub comments.

## Contract Shape

The package is serialized as `issue_finder_task_package` with `version = 3` and these current sections:

```text
issue
source
evidence
llm_confirmation
human_review
user_profile_snapshot
workspace_policy
context_pack
validation_hints
memory_context
expected_outputs
callback_policy
reproduction_contract
success_criteria
change_budget
environment_contract
interaction_policy
session_context
outcome_contract
```

`src/dispatch/task_package.rs` owns the typed DTOs and builder. `src/dispatch/packaging.rs` owns handoff import, human review approval, and package artifact creation.

## Reproduction

`reproduction_contract` expresses execution obligations rather than parsed reproduction steps. Issue text is too variable to extract reliable steps during package creation. The agent must read issue/context evidence, attempt reproduction when practical, record commands and observations, and report blockers such as missing setup, credentials, network, dependency installation, or unsafe commands.

## Change And Environment Boundaries

`change_budget` keeps the task scoped to a minimal patch, preferred candidate files, focused tests, and clear escalation triggers. `environment_contract` records workspace path, branch, dirty state, probe facts, readiness, warnings, and Issue Finder's own boundary. `interaction_policy` forbids automatic maintainer contact, GitHub posting, commits, pushes, PR creation, dependency installation, and overwriting unrelated local changes without explicit approval.

## Outcome

The required result artifact is `fix_result.json`. It must include status, summary, changed files, reproduction evidence, success criteria status, validation, residual risks, failure reason when applicable, session context, and a suggested GitHub reply. Optional artifacts are `patch`, `pr_link`, `session_link`, and `validation_log`.

Existing local v2 package artifacts are not migrated in place. Re-importing and approving a ready handoff creates a v3 package artifact.
