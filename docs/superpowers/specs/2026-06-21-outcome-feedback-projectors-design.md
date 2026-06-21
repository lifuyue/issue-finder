# Outcome Feedback Projectors Design

## Context

Dispatch outcomes used to be recorded by the dispatch runtime and immediately translated into hybrid memory ingest from `src/dispatch/memory.rs`. That made the outcome path hard to reason about: dispatch owned both event recording and the interpretation that later affected memory/ranking. Offline recommendation eval also carried separate replay weighting logic, so production and eval semantics could drift.

## Design

Dispatch runtime owns only dispatch facts: run status, outcome rows, artifacts, and dispatch events. It does not write hybrid memory and does not interpret outcomes as ranking or agent-selection policy.

Memory owns outcome feedback projection. The memory projector reads recorded dispatch outcomes through the dispatch store, converts them into raw memory events idempotently, and projects typed priors:

- `issue_quality_prior`: contribution outcomes that say similar issues are more or less likely to be worth selecting.
- `execution_friction_prior`: dependency, workspace, external service, or validation friction that can cool execution likelihood without saying the issue itself is low quality.
- `agent_suitability_prior`: agent/task evidence used for dispatch planning, not feed ranking.

Runtime failures and contribution outcomes are intentionally separated. Environment, tool, and agent runtime failures must not become issue-quality failures. Agent runtime errors project to agent suitability only. Policy-blocked, user-canceled, `needs_user`, and canceled outcomes are non-signals for ranking.

## Data Flow

1. `DispatchRuntime::record_dispatch_outcome` persists the normalized outcome and dispatch event.
2. Memory commands call the memory-owned projector sync before dreaming.
3. Memory dreaming turns raw dispatch events into candidate typed hints.
4. Live feed ranking still consumes only approved, pinned, or deprioritized memory hints through the existing memory consumption path.
5. Offline recommendation eval calls the same projector logic to compute deterministic replay adjustments.

Raw outcomes never directly mutate live feed scores.

## Testing

Coverage focuses on:

- Runtime outcome recording not writing hybrid memory directly.
- Projector classification for issue quality, execution friction, and agent suitability.
- Recommendation eval fixtures for runtime-vs-quality, environment friction, agent mismatch, and positive/negative dispatch outcomes.
- Tool output and fixture docs that no longer describe dispatch outcome recording as a memory-writing operation.
