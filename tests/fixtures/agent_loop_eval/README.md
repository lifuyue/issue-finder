# Agent Loop Evaluation Fixtures

This directory contains deterministic offline fixtures for the next-stage agent loop. The fixtures exercise lifecycle contracts across recommendation, dispatch, package creation, GitHub projection, session continuity, and memory governance without contacting GitHub, LLM services, native agent servers, user workspaces, or generated user state.

## Layout

```text
agent_loop_eval/
  schema.json
  samples.json
```

## Family Responsibilities

- `runtime_vs_quality`: runtime execution outcomes are recorded as dispatch/memory signals without rewriting issue quality.
- `package_quality`: package v2 artifacts must expose enough stable contract shape for execution agents and result import.
- `lifecycle_reactivation`: prior feedback can be partially recovered by issue updates, new comments, and maintainer activity.
- `github_interaction_policy`: GitHub writes are draft/approval/retry gated and use fake writers in eval.
- `session_continuity`: selected native sessions are resumed and remain the continuity anchor for dispatch execution.
- `memory_governance`: candidate/profile hints, memory-off mode, and suppressed scopes do not silently drift ranking/profile behavior.

Samples should describe observable contract behavior. Do not duplicate internal owner policy weights, call real services, or include private state paths.
