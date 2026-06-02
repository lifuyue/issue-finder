# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust 2021 CLI crate. The binary entry point is `src/main.rs`, and reusable logic is exported from `src/lib.rs`. Command parsing lives in `src/cli.rs`; workflow orchestration is in `src/workflow.rs`; state paths, inbox, reports, GitHub access, workspace prep, scoring, scanning, config, doctor checks, and LLM support are split across matching `src/*.rs` modules. Integration tests are in `tests/`. Design notes live under `docs/specs/`. The ignored `reference/` directory is external reference material.

## Build, Test, and Development Commands

- `cargo build`: compile the debug binary at `target/debug/patchbay`.
- `cargo run -- doctor`: run the CLI locally with readiness checks.
- `cargo test`: run all unit and integration tests.
- `cargo clippy --all-targets -- -D warnings`: enforce lint cleanliness for all targets.
- `cargo fmt --all`: format the crate before committing.
- `cargo install --path .`: install this checkout as `patchbay`.

For isolated manual runs, set `PATCHBAY_HOME=/tmp/patchbay-demo` so generated state does not touch `~/.patchbay`.

## Coding Style & Naming Conventions

Use `rustfmt`; keep code idiomatic and explicit. Prefer small modules aligned with existing responsibilities over broad utility files. Use `snake_case` for functions, variables, modules, and test names; `PascalCase` for structs, enums, and traits. CLI flags should use clear kebab-case names via `clap`, such as `--refresh` or `--date`. Keep comments sparse and useful, especially around safety boundaries.

## Testing Guidelines

Tests use Rust’s built-in test framework with `tokio::test` for async workflows. Add integration coverage in `tests/` for user-visible workflows, local state layout, GitHub API behavior, and workspace preparation. Prefer `tempfile` and `PATCHBAY_HOME`-style isolation. Name tests by behavior, for example `scout_uses_mocked_github_search_responses`.

## Commit & Pull Request Guidelines

Recent history uses short imperative summaries, sometimes with a conventional prefix, for example `Fix daily failure handling and workspace branch checks` or `docs: add Patchbay CLI Rust design`. Keep commits focused and mention the affected workflow when useful. Pull requests should include a concise description, tests run, linked issue if applicable, and screenshots only when generated Markdown or reports are relevant.

## Security & Configuration Tips

Do not commit tokens, `.env` files, generated Patchbay state, or target workspace changes. GitHub and LLM credentials belong in environment variables or `~/.patchbay/config.toml`. Preserve the project’s conservative boundary: Patchbay may prepare local workspaces and write handoff artifacts, but it should not modify target repo source, install dependencies, commit, push, or create PRs.
