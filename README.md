# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <strong>Issue Finder</strong> helps you find and complete worthwhile GitHub issues. This repository includes a standalone Codex skill that runs discovery, workspace preparation, implementation, and validation in your current session.
</p>

---

## Codex skill: install and use

The repository ships a complete, installable skill at
[`skills/issue-finder/SKILL.md`](./skills/issue-finder/SKILL.md), with its helper
script and Codex UI metadata alongside it. **This is the repository's direct
skill integration:** install or link that folder into a Codex skill directory,
then invoke `$issue-finder`. It needs no Rust binary, MCP server, plugin, second
Codex session, or Issue Finder approval workflow.

Prerequisites: Python 3.9+, Git, [GitHub CLI](https://cli.github.com/), and Codex
with local skill support. Authenticate GitHub once:

```bash
gh auth login
gh auth setup-git
```

From a checkout of **this repository**, install the whole folder for your user:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/issue-finder "$HOME/.agents/skills/issue-finder"
```

Use this copy command for a fresh installation; if the destination already
exists, update the existing installation deliberately instead of nesting another
copy. For development, use an absolute symlink instead of copying:

```bash
ln -s "$PWD/skills/issue-finder" "$HOME/.agents/skills/issue-finder"
```

Alternatively, integrate it into just one target repository by copying the same
folder to `<target-repo>/.agents/skills/issue-finder/`. Choose one scope to avoid
duplicate skills. The source `skills/` folder is a distributable package, not an
automatic discovery location. These user/repository paths and symlinks follow
the [official Codex skill discovery documentation](https://developers.openai.com/codex/skills/#where-to-save-skills).
If the skill does not appear, restart Codex.

Optionally choose a persistent contribution directory:

```bash
export ISSUE_FINDER_WORKSPACE_ROOT="$HOME/Code/contributions"
```

This is also the default. Keep it outside existing repositories and make sure
your Codex host permits writes there and GitHub network access. Installation
does not grant sandbox permissions or install project dependencies.

Examples in Codex:

```text
$issue-finder Find and complete one suitable issue in owner/repo.
$issue-finder Complete https://github.com/owner/repo/issues/123.
$issue-finder Recommend five issues in this repository; do not clone or edit.
$issue-finder Continue the prepared task in /absolute/path/to/workspace.
```

The current session selects and reviews the issue, implements it, and validates
the result. The bundled script provides only `scout`, `prepare`, and `finish`.
Normal operation has no extra candidate, plan, or review confirmation; user scope,
target repository instructions, and host permissions still apply. Commit, push,
PR creation, and GitHub comments require user authorization and are never done
by the helper. See [skill integration and behavior](./docs/skills.md) for command
examples, recovery, and limitations.

## Existing Rust CLI

The Rust CLI remains available as the existing handoff/dispatch implementation
and a migration comparison baseline. It is not a dependency or fallback for the
standalone skill; the following commands and architecture docs describe the CLI.

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

- [**Bundled Issue Finder skill**](./skills/issue-finder/SKILL.md)
- [**Skill installation, integration, and behavior**](./docs/skills.md)
- [**Usage guide**](./docs/usage.md)
- [**Agent loop architecture**](./docs/agent-loop-target-architecture.md)
- [**Agent-safe preparation runtime**](./docs/agent-safe-preparation-runtime.md)
- [**Safe probes**](./docs/safe-probes.md)
- [**Historical design archive**](./docs/superpowers/README.md)
- [**Repository guidance for coding agents**](./AGENTS.md)

## Development

```bash
python3 -m unittest discover -s tests -p 'test_skill_native.py' -v
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

This repository is licensed under the [MIT License](./LICENSE).
