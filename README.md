# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <strong>Issue Finder</strong> finds GitHub issues worth handing to coding agents, prepares local context, and coordinates approval-gated dispatch while Issue Finder itself stays out of target code edits.
</p>

<p align="center">
  <img src="./docs/assets/issue-finder-splash.svg" alt="Issue Finder workflow" width="88%" />
</p>

---

## Quickstart

### Installing and running Issue Finder

```bash
cargo install issue-finder
```

Configure GitHub access and check local readiness:

```bash
export GITHUB_TOKEN="$(gh auth token)"
issue-finder init
issue-finder doctor
```

Find candidates and prepare a handoff:

```bash
issue-finder scout --limit 10
issue-finder scout --repo owner/repo --limit 10
issue-finder prepare owner/repo#123
issue-finder handoff <inbox-id> --print
```

Issue Finder writes local state under `~/.issue-finder` by default. Use `ISSUE_FINDER_HOME=/tmp/issue-finder-demo` for isolated runs.

### Dispatch and tools

Issue Finder includes an approval-gated dispatch control plane for native agent sessions, A2A task artifacts, and GitHub comment projection. Start with the [usage guide](./docs/usage.md) for the current command flow.

Issue Finder also exposes a JSON tool contract for coding agents:

```bash
issue-finder tools list
```

## Docs

- [**Usage guide**](./docs/usage.md)
- [**Agent-safe preparation runtime**](./docs/agent-safe-preparation-runtime.md)
- [**Safe probes**](./docs/safe-probes.md)
- [**Historical design archive**](./docs/superpowers/README.md)
- [**Repository guidance for coding agents**](./AGENTS.md)

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

This repository is licensed under the [MIT License](./LICENSE).
