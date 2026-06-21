# Worktree Thread Orchestration Design

Date: 2026-06-20

Status: Draft for review

## Summary

Issue Finder needs a repeatable way to decompose the next architecture push into multiple Codex worktree threads without letting the branches invent incompatible state models. The next work should be coordinated by a main orchestration thread, but planned and implemented in four isolated worktree child threads.

The orchestration model is intentionally two-gated:

1. Child threads first run brainstorming and produce a focused design or execution plan.
2. After the main thread and user approve the plan, the same child thread continues implementation in its own worktree branch.

This design is about development workflow, not product runtime behavior. It defines how to distribute specs, create worktree sessions, review child plans, approve implementation, and merge finished work without committing directly on `main`.

## Current Context

The repository is currently at:

```text
6a18f19 Add agent dispatch control plane runtime (#24)
7ec0c40 Add hybrid contribution memory (#23)
```

Completed capabilities:

- Dispatch control plane skeleton:
  - SQLite dispatch store.
  - Agent profiles and capabilities.
  - Issue tasks.
  - Dispatch runs.
  - Agent session links.
  - Artifacts.
  - Approval requests.
  - Codex app-server adapter.
  - A2A import/export.
  - GitHub draft/post approval projection.
  - CLI and JSON tool contract surfaces.
- Hybrid contribution memory:
  - Raw event ledger.
  - Nodes, indexes, graph, activation, write-back, dreaming, hints, controls.
  - Approved memory hint consumption in ranking and handoff.
- Recommendation foundation:
  - Recommendation engine.
  - Feedback events.
  - Feed ranking.
  - Quality policy.
  - Offline recommendation eval fixtures.
  - Staged recommendation quality roadmap.

The system still lacks a complete vertical contribution loop and a full generic agent substrate. The next work should preserve Issue Finder's vertical product boundary instead of trying to rebuild a general Codex clone.

## External Alignment

This orchestration design follows current Codex and Agents direction without trying to reimplement those systems inside Issue Finder:

- Codex CLI is a local coding agent that works in a selected directory and can read, edit, and run code locally: https://developers.openai.com/codex/cli
- Codex app-server exposes thread/turn operations, compaction, background terminal cleanup, and related event notifications: https://developers.openai.com/codex/app-server
- Codex skills use progressive disclosure: metadata is visible first, and full `SKILL.md` instructions are loaded only when the skill is selected: https://developers.openai.com/codex/skills
- Codex security guidance separates sandbox boundaries from approval policy: https://developers.openai.com/codex/agent-approvals-security
- Codex app local environments support setup scripts for worktrees: https://developers.openai.com/codex/app/local-environments
- Agents SDK orchestration supports code-driven and LLM-driven multi-agent flows: https://openai.github.io/openai-agents-python/multi_agent/
- Agents SDK handoffs are useful when a specialist agent should own the next part of a workflow: https://openai.github.io/openai-agents-python/handoffs/

The repository workflow should reuse Codex worktree threads and skills rather than encoding a parallel orchestration runtime in Issue Finder product code.

## Goals

- Create a main-thread coordination model for four independent worktree child threads.
- Require every child thread to run the `brainstorming` skill before implementation.
- Make each child thread produce a plan that can be reviewed for cross-branch conflicts.
- Allow child threads to continue implementation in their own worktree only after plan approval.
- Keep all branch work off `main` until explicitly reviewed and merged.
- Encourage clean replacement and large targeted refactors where they simplify the model.
- Prevent glue-code accumulation across workflow, tool runtime, dispatch, memory, and recommendation boundaries.
- Define shared branch names, prompt templates, review gates, and merge policy.

## Non-Goals

- Do not implement the four feature streams in this spec.
- Do not create the child threads until the orchestration spec is reviewed.
- Do not commit this design directly on `main`.
- Do not allow child threads to auto-merge to `main`.
- Do not let planning threads skip the brainstorming design gate.
- Do not preserve legacy behavior merely for local backward compatibility if it blocks a cleaner design.

## Engineering Principle: Refactor Instead Of Glue

Issue Finder is still early enough that large refactors are acceptable when they clarify ownership, delete stale paths, or prevent old and new designs from coexisting indefinitely.

All child threads must follow these rules:

- Prefer clean replacement and targeted large refactors over compatibility glue.
- Do not duplicate policy rules across workflow, tool runtime, CLI adapters, JSON tools, or tests.
- Do not add temporary bridge modules unless the plan names the owner module, removal condition, and tests proving the replacement works.
- If an existing boundary prevents the correct implementation, propose the boundary change during brainstorming before implementation.
- Keep shared rules centralized:
  - Prepare gate policy stays in `src/prepare_gate.rs`.
  - Dispatch state, approval, adapter, and artifact rules stay in `src/dispatch/*`.
  - Recommendation discovery, ranking, feedback, and eval rules stay in `src/recommendation/*`.
  - Memory authority and memory consumption rules stay in `src/memory/*`.
  - Tool schema and runtime dispatch should delegate to owner modules, not copy their rules.

This principle is a hard acceptance criterion for every child plan.

## Thread Topology

The main orchestration thread owns architecture decisions and cross-branch coordination. Four child worktree threads own focused design and implementation streams.

```text
main orchestration thread
  -> fuyue/vertical-loop-v1
  -> fuyue/dispatch-runtime-next
  -> fuyue/recommendation-feedback-loop
  -> fuyue/thread-worktree-orchestration
```

Every child thread starts from latest `main` in an independent worktree. If the Codex thread creation API can only choose a starting branch, use `main` as the starting state and have the child create or switch to its assigned `fuyue/*` branch before writing files.

## Child Thread Scopes

### `fuyue/vertical-loop-v1`

Purpose: complete the first vertical contribution loop.

Scope:

- Structured LLM confirmation DTO.
- IssueTaskPackage v2 fields.
- Human review inbox or CLI review flow.
- Dispatch approval decision points.
- Outcome inputs required by memory and recommendation.

Non-scope:

- Do not design a general event bus.
- Do not rewrite recommendation ranking.
- Do not implement Codex adapter internals.

Likely owner modules:

- `src/llm_review.rs`
- `src/handoff.rs`
- `src/context_pack.rs`
- `src/dispatch/packaging.rs`
- `src/dispatch/model.rs`
- `src/dispatch/tools.rs`
- `tests/tools_contract.rs`

### `fuyue/dispatch-runtime-next`

Purpose: mature the dispatch skeleton into a reliable runtime substrate.

Scope:

- Unified dispatch event bus semantics.
- Transcript model and session replay model.
- Tool/action policy engine.
- Adapter compatibility and capability probing cache.
- Trace/timeline query surfaces.
- Approval latency and failure taxonomy.

Non-scope:

- Do not own issue recommendation quality.
- Do not design human dashboard UI.
- Do not alter prepare gate rules.

Likely owner modules:

- `src/dispatch/runtime.rs`
- `src/dispatch/store.rs`
- `src/dispatch/model.rs`
- `src/dispatch/execution.rs`
- `src/dispatch/session_ops.rs`
- `src/dispatch/adapters/*`
- `src/dispatch/tools.rs`

### `fuyue/recommendation-feedback-loop`

Purpose: feed dispatch outcomes back into discovery, ranking, memory, and eval.

Scope:

- Dispatch outcome taxonomy.
- Failure reason model.
- Agent performance by task class.
- Outcome-to-memory ingestion rules.
- Outcome-to-ranking hint consumption.
- Offline fixtures and eval report changes.

Non-scope:

- Do not execute native agents.
- Do not modify Codex app-server adapter behavior.
- Do not build the human review UI.

Likely owner modules:

- `src/recommendation/*`
- `src/memory/ingest.rs`
- `src/memory/consumption.rs`
- `src/dispatch/memory.rs`
- `tests/fixtures/recommendation_eval/*`
- `tests/recommendation_eval.rs`

### `fuyue/thread-worktree-orchestration`

Purpose: productize this multi-thread development method as repository collaboration guidance.

Scope:

- Branch naming rules.
- Worktree child prompt templates.
- Brainstorming gate.
- Plan approval gate.
- Implementation approval gate.
- Merge order and conflict policy.
- Cross-thread review checklist.
- Documentation updates for coding agents.

Non-scope:

- Do not implement business feature streams.
- Do not own dispatch, recommendation, or memory product behavior.

Likely owner files:

- `AGENTS.md`
- `docs/superpowers/specs/*`
- Possibly a new `docs/development/*` guide if the guidance grows beyond spec form.

## Child Thread Creation Protocol

The main thread should call `list_projects` and select:

```text
projectId: /Users/lifuyue/Projects/issue-finder
```

Then create one child thread per stream with a project worktree target:

```text
target:
  type: project
  projectId: /Users/lifuyue/Projects/issue-finder
  environment:
    type: worktree
    startingState:
      type: branch
      branchName: main
```

The initial prompt assigns the branch name. If the thread starts on a generated worktree branch, the child must create or switch to the assigned branch before committing.

The main thread records for each child:

```text
threadId or pendingWorktreeId
branch
scope
spec path
phase: brainstorming | plan-review | implementation | validation | ready-to-merge
```

If `create_thread` returns `pendingWorktreeId`, the main thread reports it and later uses thread listing/reading tools to continue coordination after the worktree thread appears.

## Required Child Prompt Template

Each child thread receives a prompt with this structure:

```text
You are an Issue Finder worktree child thread for <scope>.

Read:
- docs/superpowers/specs/2026-06-20-worktree-thread-orchestration-design.md
- docs/superpowers/specs/2026-06-18-agent-dispatch-control-plane-design.md
- docs/superpowers/specs/2026-06-18-hybrid-contribution-memory-design.md
- relevant source and tests for your scope

You MUST use the brainstorming skill before any implementation.

Do not write implementation code yet.
Do not add glue code to preserve stale boundaries.
Prefer clean replacement and targeted large refactors when they simplify the model.

First output a focused design/implementation plan covering:
- goal
- non-goals
- current completed behavior
- proposed architecture
- owner modules
- predicted file changes
- data model changes
- tool contract or CLI changes
- tests and evals
- likely conflicts with other worktree threads
- staged implementation steps
- validation commands
- open questions

Wait for main-thread/user approval before implementation.
After approval, continue implementation in this same worktree branch.
Do not merge to main.
```

Each prompt should also include the child-specific scope from this spec.

## Gate Model

### Gate 1: Brainstorming Gate

Child thread must prove it used brainstorming by producing a design or plan before implementation.

The plan must explicitly mention:

- What current code already does.
- What the branch will change.
- What it will not change.
- Which modules it owns.
- Which shared modules it may touch.
- How it avoids glue-code accumulation.

### Gate 2: Main-Thread Plan Review

The main thread reads every child plan with `read_thread` and reviews:

- Scope overlap.
- Shared schema conflicts.
- Duplicate policy logic.
- Unclear ownership.
- Missing tests.
- Missing migration or deletion plan.
- Inconsistent terminology across task package, dispatch, memory, and recommendation.

The main thread may approve, request revisions, or sequence one branch before another.

### Gate 3: Implementation Approval

Only after plan approval does the main thread instruct a child to implement.

Implementation approval should be explicit:

```text
Approved to implement the reviewed plan in your current worktree branch.
Do not expand scope without returning to plan review.
```

### Gate 4: Validation Gate

Each child thread must run focused validation before reporting ready:

```text
cargo fmt --all
cargo test <focused tests>
cargo clippy --all-targets -- -D warnings
```

Branches that modify recommendation behavior must also run:

```text
cargo test --test recommendation_eval
```

Branches that modify tool contracts must run:

```text
cargo run -- tools list
```

The main thread runs final full validation after merge sequencing.

## Merge Strategy

The preferred merge order is:

1. `fuyue/thread-worktree-orchestration`
2. `fuyue/dispatch-runtime-next`
3. `fuyue/vertical-loop-v1`
4. `fuyue/recommendation-feedback-loop`

Rationale:

- Collaboration rules should stabilize before feature work.
- Runtime substrate should land before vertical workflow consumes it.
- Recommendation feedback should land after outcome semantics are stable.

This order can change if child plans identify a cleaner dependency path.

The main thread owns merge decisions. Child threads must not merge to `main`, push PRs without approval, or resolve cross-branch conflicts independently.

## Conflict Policy

Potential conflict hotspots:

- `src/dispatch/model.rs`
- `src/dispatch/store.rs`
- `src/dispatch/tools.rs`
- `src/tool_runtime.rs`
- `src/tool_specs.rs`
- `src/memory/model.rs`
- `src/memory/ingest.rs`
- `src/recommendation/engine.rs`
- `tests/tools_contract.rs`

Rules:

- One branch should own any schema/table change.
- Tool contract additions should be grouped through owner modules.
- Shared DTO changes must include tests at the owner boundary.
- If two child plans need the same file, the main thread decides whether to sequence, split ownership, or require a shared pre-branch refactor.

## Documentation Requirements

Every child branch that changes behavior must update or create docs in the same branch. Outdated docs should be updated or removed in the same change.

Required doc updates by branch:

- `thread-worktree-orchestration`: repository collaboration guidance.
- `dispatch-runtime-next`: dispatch runtime design updates.
- `vertical-loop-v1`: vertical contribution loop and package v2 design.
- `recommendation-feedback-loop`: recommendation eval and outcome feedback design.

Docs must not leave contradictory old/new descriptions for later contributors to reconcile.

## Testing Strategy

The orchestration branch itself should only need documentation review and normal markdown/file checks.

Feature branches should use the repository's existing validation profile:

```text
cargo fmt --all
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- tools list
```

Recommendation-affecting work must maintain `tests/fixtures/recommendation_eval/` and document whether new samples were added.

Dispatch-affecting work must include integration tests using mock/local state, not real Codex, real GitHub writes, or real network dependencies.

Memory-affecting work must use deterministic local state and must not require external embeddings, LLMs, or GitHub access in automated tests.

## Risks

- Child threads may design overlapping schema changes.
  - Mitigation: main-thread plan review before implementation.
- Branches may pass focused tests but fail together.
  - Mitigation: main-thread merge sequencing and full final validation.
- Worktree thread creation may return `pendingWorktreeId` instead of `threadId`.
  - Mitigation: record pending ids and reconcile with thread listing.
- A child may drift into implementation before design approval.
  - Mitigation: prompt hard gate and main-thread review.
- Large refactors may create wide diffs.
  - Mitigation: require owner module, deletion plan, and focused tests.

## Acceptance Criteria

This orchestration design is accepted when:

- The spec is committed on a non-`main` branch.
- The user has reviewed and approved the spec.
- Four child thread prompts can be generated directly from this spec.
- Each child prompt requires brainstorming before implementation.
- Each child branch starts from latest `main` in its own worktree.
- The clean-refactor/no-glue principle is part of every child plan gate.
- Main-thread review and implementation approval gates are explicit.

## Next Step After Approval

After this spec is reviewed and approved, the main thread should create four worktree child threads with `create_thread`. Each child should receive its scope-specific prompt and begin with brainstorming only.
