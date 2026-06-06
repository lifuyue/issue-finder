# Recommendation Evaluation Fixtures

This directory contains deterministic offline fixtures for recommendation ranking evaluation. These fixtures are used by tests and must not access GitHub, LLM services, user workspaces, tokens, or generated Issue Finder state.

## Layout

```text
recommendation_eval/
  schema.json
  datasets/
    core_quality.json
    profile_frontend.json
    profile_backend_rust_go.json
    profile_python_data_cli.json
    profile_ai_agent_tools.json
    profile_devops_infra.json
    source_trust.json
    feedback_replay.json
```

## Sample Shape

Each sample describes the minimum evidence needed to reconstruct an `EnrichedIssue`, rank it with the configured profile, and compare the result with a human expectation.

Required fields:

- `id`: stable sample id.
- `issue`: issue title, body, labels, repo, and number.
- `repository`: repo language, influence, topics, and activity hints.
- `expected.quality`: one of `excellent`, `good`, `weak`, `reject`.
- `expected.behavior`: one of `visible_top`, `visible`, `visible_lower`, `hidden`, `fallback_candidate`.
- `expected.reasons`: human-readable explanation for the expectation.

Prefer `createdAgeDays`, `updatedAgeDays`, and `pushedAgeDays` over fixed dates when the sample is not about an exact historical date. The evaluator converts these relative values to RFC3339 timestamps at runtime, keeping tests stable over time.

## Adding Samples

Add a sample when a live run exposes a ranking failure, source trust failure, fallback failure, or feedback/cooldown regression. The sample should be small but specific enough that a reviewer can understand why it should pass or fail without reading external GitHub pages.

Do not copy complete GitHub API payloads. Do not include tokens, private user data, generated local state, or temporary cache paths.

