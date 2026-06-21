# Worktree Thread Orchestration

This guide is the operational source for Issue Finder multi-worktree child threads. Dated specs can record design history, but child prompts, approval gates, and merge policy live here so future worktrees starting from `main` can find the current rules.

## Goals

- Let the main thread split large work into focused child worktrees without losing owner boundaries.
- Make every child thread show design evidence before implementation.
- Keep branch, approval, merge, and conflict handling predictable.
- Preserve Issue Finder's module ownership model instead of adding glue code across stale boundaries.

## Non-goals

- This guide does not define dispatch, recommendation, memory, prepare gate, GitHub, A2A, or tool runtime product behavior.
- This guide does not replace module-level `AGENTS.md` files.
- This guide does not allow child threads to merge to `main`, create pull requests, or bypass native approval systems.
- This guide does not require maintaining duplicate child prompt templates in dated design specs.

## Owner Boundaries

Shared rules stay with their owners:

- Prepare gate policy: `src/prepare_gate.rs`.
- Dispatch runtime, session links, approvals, A2A, GitHub projection, and dispatch tools: `src/dispatch/*`.
- Recommendation ranking, quality, freshness, feedback, and eval fixtures: `src/recommendation/*` and `tests/fixtures/recommendation_eval/`.
- Contribution memory: `src/memory/*`.
- Tool schema/runtime: the owner module exports schema or behavior, and adapter code delegates to it.
- Repository collaboration process: root `AGENTS.md` and this guide.

Documentation may describe these boundaries. It must not copy product policy into a different module or preserve obsolete behavior with compatibility glue.

## Branch Naming

Child worktree branches use:

```text
fuyue/<stream-name>
```

Rules:

- The main thread assigns the branch name in the delegation prompt.
- A child worktree may start from detached `HEAD` at latest `main`.
- Before any file write or commit, the child must create or switch to the assigned branch.
- Do not commit on detached `HEAD`.
- Do not commit on `main`.
- Do not merge to `main` from a child thread.
- Do not split, rename, or reuse the assigned branch without main-thread approval.

## Child Thread Flow

```text
main delegation
  -> child inspects current context
  -> brainstorming-compliant plan
  -> plan approval gate
  -> implementation approval gate
  -> switch/create assigned branch
  -> scoped implementation
  -> validation
  -> commit on assigned branch
  -> main-thread review and merge ordering
```

The main thread can approve both gates in one explicit message. Without that explicit approval, the child stops after the plan.

## Child Prompt Template

Use this template when creating a child worktree thread:

```text
You are the Issue Finder worktree child thread for <stream>.

Assigned branch: fuyue/<stream>.
Start from latest main. Do not write files until the plan is approved.
Use the authoritative brainstorming protocol before implementation.

Scope:
- <owned files/modules/docs>

Non-scope:
- <explicit excluded owner modules and behaviors>

Current completed context:
- <important commits, branches, or completed features>

First deliverable:
Brainstorming-compliant plan covering context evidence, 2-3 approaches,
recommended design, owner boundaries, predicted file changes, data model changes,
CLI/tool contract changes, tests/evals, likely conflicts, implementation stages,
validation commands, and main-thread decisions needed.

Stop before implementation until the main thread explicitly approves.
```

## Brainstorming Gate

Before implementation, a child thread must produce a plan with:

- Context evidence from actual project inspection: files, docs, current branch state, and recent commits.
- Two or three approaches with trade-offs.
- One recommended approach.
- Recommended design covering architecture, components, data flow, error handling, and testing.
- Clear non-goals.
- Owner boundaries.
- Predicted file changes and explicit statements for data model, CLI, and tool contract impact.
- Cross-thread conflict risks.
- Implementation stages.
- Validation commands.
- Main-thread decisions needed, with recommended defaults.

The child must not write code, scaffold files, create implementation modules, or switch into implementation during this gate.

### Missing Brainstorming Skill Fallback

Worktree isolation can hide local skill files such as `.agents/skills/brainstorming/SKILL.md`. That is not a waiver. If the skill is missing, the child must follow the protocol in this guide and state that it used the authoritative fallback.

## Approval Gates

### Plan Approval Gate

The main thread reviews the brainstorming-compliant plan. It may:

- Approve the plan.
- Request plan revisions.
- Reject the stream.
- Approve the plan and implementation in the same explicit message.

Until this gate passes, the child thread must not implement.

### Implementation Approval Gate

Implementation approval permits scoped file edits. Before writing files, the child must:

- Confirm or create the assigned `fuyue/<stream-name>` branch.
- Check worktree status.
- Preserve unrelated user changes.
- Re-read relevant owner instructions when touching nested directories.

Implementation approval does not permit:

- Editing out-of-scope runtime behavior.
- Duplicating prepare gate, dispatch, recommendation, memory, or tool policy.
- Creating PRs.
- Merging to `main`.
- Resetting or reverting unrelated changes.

## Documentation Ownership

Keep mandatory, short rules in root `AGENTS.md`. Keep the complete workflow, templates, gates, validation expectations, and cross-thread checklist in this guide.

Do not import the dated worktree orchestration design spec by default. The development guide is the maintained operational source. A dated spec can link here if a later design update needs historical context.

## Merge Order And Conflict Policy

Recommended merge order:

1. Shared process and documentation branches that unblock future child threads.
2. Owner-scoped runtime branches after their tests pass.
3. Broad product or vertical branches after conflicts with owner-scoped streams are resolved.

Conflict ownership:

- `AGENTS.md` process rules and this guide: thread-worktree-orchestration stream.
- Dispatch runtime and dispatch-facing docs: dispatch-runtime stream.
- Recommendation logic, eval fixtures, and eval docs: recommendation-feedback stream.
- Product vertical workflow docs and surfaces: the assigned vertical stream.
- Prepare gate policy: `src/prepare_gate.rs`.
- Memory behavior: `src/memory/*`.

When resolving conflicts:

- Preserve the stricter approval or safety rule.
- Prefer owner module behavior over duplicated descriptions elsewhere.
- Update or delete stale docs instead of leaving conflicting instructions.
- Do not solve conflicts by adding compatibility glue across obsolete boundaries.

## Cross-thread Review Checklist

Use this checklist before accepting a child thread result:

- Did the child inspect current project context and cite concrete files, docs, branch state, or commits?
- Did the child compare alternatives and recommend one?
- Did the child pass plan approval before implementation?
- Did the child create or switch to the assigned branch before file writes?
- Did the child avoid detached `HEAD` and `main` commits?
- Did the child stay inside the approved scope?
- Did the child preserve owner module boundaries?
- Did the child avoid duplicating product policy?
- Did documentation changes remove or update stale contradictory text?
- Did the child run the agreed validation commands?
- Did the final report include branch, commit, changed files, validation results, failures, and expected merge conflicts?

## Cross-thread Contracts

### Vertical streams

Vertical streams own user-facing workflow or product-surface changes assigned by the main thread. They must list owner modules before implementation, update user-facing docs when behavior changes, and avoid weakening orchestration gates.

### Dispatch-runtime streams

Dispatch-runtime streams own `src/dispatch/*`, dispatch CLI and tool specs, dispatch tests, Codex app-server behavior, A2A mapping, GitHub projection, and dispatch docs. Other streams can reference dispatch boundaries but must not redefine dispatch state machines or adapter policy.

### Recommendation-feedback streams

Recommendation-feedback streams own `src/recommendation/*`, recommendation eval fixtures, feedback, freshness, quality, ranking, and recommendation eval reports. Any change to discovery, fallback, feed ranking, quality policy, freshness, or feedback cooldown must maintain the eval fixtures or document why no new fixture is needed.

### Memory streams

Memory streams own `src/memory/*`, memory evals, raw event semantics, activation, dreaming, hints, controls, and write-back behavior. Other streams may mention memory as an owner boundary but must not change memory authority policy.

## Error Handling

- Missing assigned branch: create it only after implementation approval, then continue.
- Dirty worktree: inspect whether changes are related; do not revert unrelated user changes.
- Missing brainstorming skill: use the fallback protocol in this guide.
- Ambiguous ownership: list the issue under main-thread decisions needed, recommend a default, and stop before implementing cross-owner behavior.
- Validation failure: report the command, failure reason, and whether the failure is related to the change.
- Conflicting documentation: make one source authoritative and remove or update stale contradictory text.

## Validation Expectations

For documentation/process streams, run at least:

```bash
rg -n "brainstorm|plan approval|implementation approval|worktree|child thread|thread-worktree|fuyue/" AGENTS.md docs src tests
rg -n "prepare gate|prepare_gate|recommendation eval|dispatch|memory" AGENTS.md docs/development docs/superpowers/specs src/AGENTS.md tests/AGENTS.md
cargo fmt --all -- --check
```

When time allows, also run:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

If full tests or clippy are skipped, the child final report must explain why.

Recommendation eval is required only when the change affects discovery, fallback, feed ranking, quality policy, freshness, or feedback cooldown.

## Final Report Requirements

Child thread final responses after implementation must include:

- Branch.
- Commit hash.
- Changed files.
- Validation results.
- Any failed command and reason.
- Expected merge conflicts with other threads.
