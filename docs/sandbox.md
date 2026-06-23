# Sandbox & Approvals

Issue Finder does not implement an operating system sandbox or a model tool loop. During prepare it writes a handoff contract that downstream agent runtimes can read and enforce with their own sandbox and approval systems. The dispatch control plane can later start or resume native agent sessions only after local approvals; native runtimes still own their sandbox and approval behavior.

## Issue Finder Boundary

Issue Finder may:

- Read GitHub issue and repository metadata
- Clone or fetch the target repository
- Create or checkout a local Issue Finder branch
- Scan repository files within bounded limits
- Run fixed low-risk probes
- Write Issue Finder state under `~/.issue-finder` or `ISSUE_FINDER_HOME`
- Track approval-gated dispatch/session/A2A/GitHub projection state

Issue Finder must not:

- Modify target repository source itself
- Install dependencies
- Run repository tests, lint, build, or project-defined scripts
- Commit, push, or create pull requests
- Reset, clean, or delete workspaces

## Agent Approval Categories

`agent-policy.json` uses three command categories:

- `allowed_low_risk`: fixed read-only probes Issue Finder already considered safe enough to run.
- `requires_user_approval`: useful commands that may execute repository code, use dependencies, take time, or need network access.
- `forbidden`: destructive or out-of-bound actions for Issue Finder handoff consumption.

Validation candidates such as `cargo test`, `npm test`, `pytest`, `go test ./...`, and `make test` are suggestions only. They are classified as `requires_user_approval`.

## Protected Paths

The policy manifest protects:

- Workspace metadata: `.git`, `.agents`, `.codex`
- Issue Finder inbox item directory
- Generated context files under the inbox item

These paths are part of the handoff boundary. They are not target source files.
