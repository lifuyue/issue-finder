# Patchbay CLI

<p align="center">
  <strong>Patchbay CLI</strong> prepares local-first contribution handoffs for developers who work with coding agents.
</p>

<p align="center">
  <img src="./docs/assets/patchbay-splash.svg" alt="Patchbay CLI workflow" width="88%" />
</p>

Patchbay finds suitable GitHub issues, ranks them with local heuristics, prepares a safe workspace, and writes a structured handoff package for tools such as Codex, Cursor, Claude Code, and Cline.

It stops before the risky parts: Patchbay does not modify target repository source, install dependencies, run validation commands, commit, push, or create pull requests.

<p align="center">
  <img src="./docs/assets/patchbay-terminal.svg" alt="Patchbay CLI terminal preview" width="88%" />
</p>

---

## Quickstart

### Install from this checkout

```bash
cargo install --path .
```

Patchbay needs Rust, Git, and a GitHub token with read access:

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

### Prepare your first handoff

```bash
patchbay init
patchbay doctor
patchbay scout --limit 10
patchbay prepare owner/repo#123
patchbay handoff <inbox-id> --print
```

For isolated local runs, keep generated state out of `~/.patchbay`:

```bash
PATCHBAY_HOME=/tmp/patchbay-demo patchbay doctor
```

## Docs

- [**Usage guide**](./docs/usage.md)
- [**Rust design notes**](./docs/specs/patchbay-cli-rust-design.md)
- [**Workflow design specs**](./docs/superpowers/specs/)
- [**Repository guidance for coding agents**](./AGENTS.md)

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```
