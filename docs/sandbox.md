# Sandbox & Approvals

Patchbay does not implement an operating system sandbox or a model tool loop. It writes a handoff contract that downstream agent runtimes can read and enforce with their own sandbox and approval systems.

## Patchbay Boundary

Patchbay may:

- Read GitHub issue and repository metadata
- Clone or fetch the target repository
- Create or checkout a local Patchbay branch
- Scan repository files within bounded limits
- Run fixed low-risk probes
- Write Patchbay state under `~/.patchbay` or `PATCHBAY_HOME`

Patchbay must not:

- Modify target repository source during prepare
- Install dependencies
- Run repository tests, lint, build, or project-defined scripts
- Commit, push, or create pull requests
- Reset, clean, or delete workspaces

## Agent Approval Categories

`agent-policy.json` uses three command categories:

- `allowed_low_risk`: fixed read-only probes Patchbay already considered safe enough to run.
- `requires_user_approval`: useful commands that may execute repository code, use dependencies, take time, or need network access.
- `forbidden`: destructive or out-of-bound actions for Patchbay handoff consumption.

Validation candidates such as `cargo test`, `npm test`, `pytest`, `go test ./...`, and `make test` are suggestions only. They are classified as `requires_user_approval`.

## Protected Paths

The policy manifest protects:

- Workspace metadata: `.git`, `.agents`, `.codex`
- Patchbay inbox item directory
- Generated context files under the inbox item

These paths are part of the handoff boundary. They are not target source files.
