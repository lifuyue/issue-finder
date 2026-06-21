# Recommendation Evaluation Datasets

Each dataset focuses on one ranking pressure. Add new samples to an existing dataset when the profile or failure mode already matches; create a new dataset only when the failure mode would make an existing dataset ambiguous.

## Dataset Responsibilities

- `core_quality.json`: universal quality gates such as open PR, claimed work, dashboard noise, no-code tasks, stale issues, and overlay-trusted small repos.
- `profile_frontend.json`: React, UI, browser, form, and component issues. Prevents generic TypeScript/backend issues from outranking real frontend work.
- `profile_backend_rust_go.json`: Rust, Go, backend, compiler, cargo, and service issues. Protects weak profiles from returning too few useful candidates.
- `profile_python_data_cli.json`: Python, data, pandas, testing, and CLI issues. Prevents unrelated TypeScript/cloud issues from polluting Python results.
- `profile_ai_agent_tools.json`: AI, LLM, agent, eval, and developer-tool issues. Filters tutorials, bounties, and generic devtools noise.
- `profile_devops_infra.json`: Kubernetes, Docker, CI, GitOps, and cloud infrastructure issues.
- `source_trust.json`: GFI trusted, overlay trusted, and global source trust behavior.
- `feedback_replay.json`: shown/read/prepared/done/dismissed/restored state behavior.
- `dispatch_outcome_replay.json`: approved dispatch outcome trends, validation failure cooldown, policy/user block exclusion, and candidate-hint inertness.
- `lifecycle_reactivation.json`: recommendation lifecycle recovery when a previously cooled issue receives new issue, comment, or maintainer activity.

## Profile Names

Use these stable profile identifiers:

- `default_cli_devtools`
- `typescript_frontend`
- `rust_backend_systems`
- `python_data_cli`
- `ai_agent_tools`
- `devops_infra`

Each sample must either inherit the dataset `profile` or set its own `profile`.
