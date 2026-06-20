# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <strong>Issue Finder</strong> finds GitHub issues worth handing to coding agents, prepares local context, tracks dispatch state, and stops before code changes.
</p>

<p align="center">
  <img src="./docs/assets/issue-finder-splash.svg" alt="Issue Finder workflow" width="88%" />
</p>

---

## Quickstart

### Installing Issue Finder

```bash
cargo install issue-finder
```

If you want your main coding agent to handle first-run setup, give it this prompt:

```text
Install cargo issue-finder locally, run `issue-finder profile bootstrap --json`,
review the report's tech stack, keyword, and project evidence, remove noise,
then update `[profile]` in `~/.issue-finder/config.toml`. Do not copy session
bodies, secrets, system prompts, or tool output into the config. Then run
`issue-finder doctor` and `issue-finder scout --limit 10` to verify.
```

Then configure GitHub access and check local readiness:

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

Inspect the local dispatch control plane:

```bash
issue-finder agents list
issue-finder agents capabilities codex
issue-finder sessions list --agent codex
issue-finder sessions sync --agent codex --limit 20
issue-finder sessions search --issue owner/repo#123
issue-finder sessions rename <session-link-id> --name "issue-finder: owner/repo#123 - short title"
issue-finder sessions fork <session-link-id>
issue-finder sessions approve <approval-request-id>
issue-finder dispatch owner/repo#123 --agent codex --new-session
issue-finder dispatch owner/repo#123 --agent codex --session <session-link-or-native-id>
issue-finder dispatch package import-handoff <inbox-id>
issue-finder dispatch approve <run-id>
issue-finder dispatch execute <run-id>
issue-finder dispatch a2a export owner/repo#123
issue-finder dispatch a2a approve <approval-request-id>
issue-finder dispatch github draft-tracking owner/repo#123
issue-finder dispatch github approve <interaction-id>
issue-finder dispatch github post <interaction-id>
issue-finder dispatch github retry <interaction-id>
```

Issue-based dispatch and projection commands import the matching ready inbox handoff into an `IssueTaskPackage` when needed. `dispatch package import-handoff` is the explicit inspection path.

Issue Finder writes local state under `~/.issue-finder` by default. Use `ISSUE_FINDER_HOME=/tmp/issue-finder-demo` for isolated runs.

### Tool Contract

Issue Finder also exposes a JSON tool contract for coding agents:

```bash
issue-finder tools list
issue-finder tools call issue-finder.scout --arguments '{"limit":10}'
issue-finder tools call issue-finder.scout --arguments '{"repo":"owner/repo","limit":10}'
```

## Docs

- [**Usage guide**](./docs/usage.md)
- [**Agent-safe preparation runtime**](./docs/agent-safe-preparation-runtime.md)
- [**Safe probes**](./docs/safe-probes.md)
- [**Repository guidance for coding agents**](./AGENTS.md)

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

This repository is licensed under the [MIT License](./LICENSE).
