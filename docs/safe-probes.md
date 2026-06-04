# Safe Probes

Safe probes are fixed, bounded commands that Patchbay may run during prepare to understand the workspace without executing project code.

Patchbay does not accept arbitrary probe strings from issues, config, LLM output, handoff files, or user-provided command text. Probe commands are represented as enum variants and expanded directly to argv arrays.

## Current Probe Set

Patchbay may run:

| Probe | Command | Purpose |
| --- | --- | --- |
| `git_status_porcelain` | `git status --porcelain` | Read dirty state |
| `git_branch_show_current` | `git branch --show-current` | Read current branch |
| `git_ls_files` | `git ls-files` | Read tracked files |
| `git_remote_get_url_origin` | `git remote get-url origin` | Read origin URL |
| `npm_pkg_get_scripts` | `npm pkg get scripts --json` | Read package script metadata |
| `pnpm_pkg_get_scripts` | `pnpm pkg get scripts --json` | Read package script metadata when pnpm is detected |

Patchbay also statically inspects manifests and discovered files to record package managers, agent instruction files, package scripts, and validation candidates.

## Commands Patchbay Does Not Run

Patchbay does not run:

- `cargo test`
- `cargo check`
- `npm install`
- `npm test`
- `pnpm install`
- `pytest`
- `make`
- Project-defined scripts
- Commands inferred directly from issue text

Those commands may appear as validation candidates with `requires_user_approval`.

## Probe Output Limits

Each probe has:

- Explicit workspace cwd
- Short timeout
- Captured stdout and stderr
- Byte and line limits
- Exit code recording
- Duration recording
- Timeout and truncation warnings
- Lossy decoding for invalid UTF-8 with a warning

Probe failures are non-fatal by default. They are written into `probe.json`, `context/probe.md`, and daily report warning fields.
