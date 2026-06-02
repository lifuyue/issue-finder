# Patchbay macOS CI Design

## Summary

Patchbay should add a small GitHub Actions CI workflow that runs the project's basic Rust quality checks on macOS. This first CI iteration is intentionally limited to validation only: it should not package binaries, publish releases, upload artifacts, or require external service secrets.

The goal is to catch formatting, lint, and test regressions before code reaches `main` while keeping the pipeline simple enough to maintain during early product development.

## Goals

- Run basic Rust validation for pull requests targeting `main`.
- Run the same validation for pushes to `main`.
- Use a macOS runner because the current primary development and verification environment is macOS.
- Match the commands documented in `README.md`.
- Avoid publishing, packaging, signing, or release automation in this iteration.
- Keep the workflow independent from GitHub API tokens, LLM API keys, and Patchbay user configuration.

## Non-Goals

- Do not build release binaries.
- Do not publish to GitHub Releases.
- Do not publish to crates.io.
- Do not upload build artifacts.
- Do not add Linux or Windows jobs in this iteration.
- Do not add dependency audit, license policy, coverage reporting, or benchmark jobs yet.
- Do not run commands against real GitHub APIs or real LLM services.

## Proposed Workflow

Add one workflow file:

```text
.github/workflows/ci.yml
```

Workflow name:

```text
CI
```

Triggers:

- `pull_request` with `main` as the base branch.
- `push` to `main`.

Runner:

```yaml
runs-on: macos-latest
```

Toolchain:

- Rust stable.
- Components:
  - `rustfmt`
  - `clippy`

Validation commands:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The command order is deliberate. Formatting should fail quickly before linting, and linting should fail before the full test suite when possible.

## Caching

Use GitHub Actions cache for Cargo dependency and build caches:

- `~/.cargo/registry`
- `~/.cargo/git`
- `target`

The cache key should include:

- operating system
- Cargo lockfile hash

Example shape:

```yaml
key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
```

This keeps CI reasonably fast without adding third-party actions beyond standard checkout/toolchain/cache steps.

## Data Flow

```text
GitHub event
  -> checkout repository
  -> install stable Rust with rustfmt and clippy
  -> restore Cargo cache
  -> check formatting
  -> run clippy with warnings denied
  -> run tests
  -> report job status to GitHub
```

No Patchbay runtime state should be checked in or persisted. Tests that need local state should continue to use temporary directories.

## Error Handling

- If formatting fails, the job should fail with the `cargo fmt` output.
- If clippy emits any warning, the job should fail because warnings are denied.
- If tests fail, the job should fail with standard Cargo test output.
- If cache restore misses, the workflow should continue normally.
- If the runner has no required toolchain component, the setup step should install it.

No fallback should bypass failed validation. CI should be a hard gate for these checks.

## Security And Secrets

The workflow should not require secrets.

The tests should not rely on:

- `GITHUB_TOKEN`
- GitHub Personal Access Tokens
- LLM API keys
- user-specific `PATCHBAY_HOME`

Existing tests use local mock servers, temporary directories, and local git repositories, which fits this boundary.

## Scope For Later Iterations

Future CI/CD phases can add:

- Linux and Windows matrix jobs.
- `cargo audit` or `cargo deny`.
- coverage reporting.
- release binaries for tagged versions.
- GitHub Release publishing.
- crates.io publishing.
- checksum generation and signing.

These should be separate design steps because they require release policy, token handling, and versioning decisions.

## Acceptance Criteria

- A pull request targeting `main` runs the macOS CI workflow.
- A push to `main` runs the same workflow.
- The workflow checks formatting with `cargo fmt --all -- --check`.
- The workflow runs `cargo clippy --all-targets -- -D warnings`.
- The workflow runs `cargo test`.
- The workflow does not package, publish, upload artifacts, or require secrets.
- The workflow can pass using the current repository state.

## Verification Plan

After implementation:

- Run the CI commands locally:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
- Inspect the workflow syntax.
- Push the branch and confirm GitHub Actions starts on the pull request.
