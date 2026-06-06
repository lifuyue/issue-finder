# Recommendation Evaluation Reports

This directory stores selected live and offline recommendation evaluation reports. These reports are product evidence for algorithm iterations; they are not CI inputs and must not contain tokens, private local state, or raw generated cache directories.

## Fixed Live Profiles

Important algorithm versions should run the same six profiles with `limit=15`, `refresh=true`, an isolated `ISSUE_FINDER_HOME`, and `recordExposure=false`:

- `default_cli_devtools`
- `typescript_frontend`
- `rust_backend_systems`
- `python_data_cli`
- `ai_agent_tools`
- `devops_infra`

Hard pass is `visible >= 11`; target pass is `visible >= 12`.

## Report Files

Each committed report directory should contain:

- `metrics.json`: structured offline/live metrics summary.
- `report.md`: human-readable review, including direct issue-content quality judgments.
- `visible.jsonl`: compact visible candidate rows for audit and future fixture extraction.

Keep detailed temporary scout outputs in `/tmp` unless they are intentionally reduced to the compact report files above.

## Offline Baseline Workflow

Generate a deterministic offline report with:

```sh
ISSUE_FINDER_RECOMMENDATION_EVAL_REPORT_DIR=/tmp/issue-finder-recommendation-eval cargo test --test recommendation_eval
```

Review the generated `metrics.json`, `report.md`, and `visible.jsonl` before copying a selected baseline into this directory. The normal test run does not write report files.

## Feeding Back Into Fixtures

When a live run finds a representative failure, add a compact sample to `tests/fixtures/recommendation_eval/datasets/` so future algorithm versions can catch the regression offline.
