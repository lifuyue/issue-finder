# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

Issue Finder helps Codex discover and assess GitHub issues. Codex selects work,
prepares the workspace, reproduces and fixes the issue, validates the change,
reviews it, and delivers the PR.

## CLI + skill

The primary CLI integration is
[`skills/issue-finder-cli/SKILL.md`](./skills/issue-finder-cli/SKILL.md). Its
current-session tool profile exposes only `scout` and `assess` for discovery,
GitHub search controls, ranking, and evidence. `scout` uses a bounded System 1
step through Codex app-server (`gpt-6-luna`, reasoning disabled) to answer
seven fixed semantic questions (`scout-semantics-v2`) before final selection. Fresh GitHub
availability checks run before screening and again before display, with backfill
within the bounded pool and API budget. The main agent owns the repair.
It needs no MCP server, dispatch setup, or interactive Issue Finder configuration.

Add this checkout as a Codex project and invoke the source skill directly:

```text
Use skills/issue-finder-cli/SKILL.md to recommend five Rust CLI issues.
Use skills/issue-finder-cli/SKILL.md to find and complete one issue in owner/repo.
```

To make it available in the skill picker, link the source package from the
project root:

```bash
mkdir -p .agents/skills
ln -s ../../skills/issue-finder-cli .agents/skills/issue-finder-cli
```

If the destination already exists, inspect and update the existing installation
instead of nesting or overwriting it. Alternatively, copy the whole
`skills/issue-finder-cli` directory to `~/.agents/skills/issue-finder-cli` for all
projects. Choose one discovery scope to avoid duplicates. Codex supports these
repository/user locations and symlinked folders; `skills/` itself is a source
package location, not an automatic discovery directory.
[Official skill discovery documentation](https://learn.chatgpt.com/docs/build-skills#where-to-save-skills)

Then invoke the skill in Codex:

```text
$issue-finder-cli Recommend five issues matching my Rust and developer-tools interests.
$issue-finder-cli Find and complete one suitable issue in owner/repo.
$issue-finder-cli Complete https://github.com/owner/repo/issues/123.
```

Git, a compatible `issue-finder` binary, and an authenticated Codex CLI for
System 1 must be available in the agent's execution environment. Install the
published crate with Cargo:

```bash
cargo install issue-finder --locked
```

Prebuilt binaries and checksums are available from the
[official stable releases](https://github.com/lifuyue/issue-finder/releases/latest).
The Cloud environment configures installation, dependencies, PATH, and supported
authentication before Codex runs. Before a compatible stable
release is published, explicitly install this checkout with
`cargo install --path . --locked`. Package version alone does not establish
compatibility; the catalog must include `sessionContractVersion: 2`:

```bash
issue-finder tools --profile session list
issue-finder system1-check
issue-finder tools call issue-finder.scout --arguments '{"limit":5}'
```

Cloud setup explicitly installs the tested Codex CLI `0.159.3` in a private npm
prefix; local use discovers and reuses the existing CLI/login. The repository
[setup script](./scripts/system1-codex.sh) supports `--cloud-install` and
`--check-only`, both followed by a real schema/model/reasoning/auth check. See the
[installation guide](./skills/issue-finder-cli/references/install.md).

Use `GH_TOKEN` as the single GitHub credential environment variable. Session
tools fall back to optional configured credentials and then the host's stored
`gh` login. `GITHUB_TOKEN` is not read by Issue Finder. Providing `GH_TOKEN`
does not require `gh`. No `init` or status preflight is required. Business calls report configuration,
authentication, and network failures directly; read access does not prove PR
creation permissions.
CLI state defaults to `~/.issue-finder`; `ISSUE_FINDER_HOME` selects an isolated
state directory.

GitHub search sort, query, pagination, and API budget are explicit tool inputs.
The CLI ranks the retrieved candidates; the agent reads their issue evidence
and chooses work that fits the request. Recommendation-only requests stop before
workspace preparation. All user interaction stays in the current agent session.
Scout defaults to four concurrent candidates, using one app-server process with
independent issue threads and one seven-question request per issue; each question
retains its criteria and material. Concurrency must be positive with no extra upper
cap. A failed candidate is isolated, with at most one retry for a retryable server
response within its original timeout. Scout exposes scoped semantic answers,
incomplete material, failures, availability facts and saved snapshots. Failed or
budget-skipped screening stays identifiable; it never silently restores semantic keyword filtering. `assess` reads fresh evidence and
runs fresh final-depth availability checks without a model request. The six-hour
semantic cache never replaces fresh GitHub facts. Discussion claims provide soft
reminders to check evidence; verified PR identities and resolution relationships
matter, while mentions, search leads and incomplete coverage retain uncertainty.
Concrete documentation, generated, event and rewarded tasks are evaluated by their
goal, scope, clarity and explicit preferences. Original eight-question snapshots and
reports remain historical; v1 replays preserve their captured outcome. See
[System 1](./docs/system1.md) and the
[reference](./docs/README.md) for tool behavior and local state.

`tools` and `mcp` default to the two-tool session profile. Recommendation scores
are advisory, not permission gates. Codex ranking ignores historical
`dismissed`, `done`, and `prepared` events while retaining automatic shown/read
history. Old event files and session task files are left on disk; session task
files are no longer read or resumed. There is no manual cross-chat dismissal or
CLI completion record in this contract.

## Legacy CLI workflows

The CLI also includes terminal commands, handoff generation, contribution
memory, JSON/MCP adapters, and a separate dispatch control plane. Dispatch
manages native agent sessions and its own approvals; the current-session skill
does not invoke it. These shared compatibility paths require explicit commands
or `--profile control` / `--profile worker`; they are outside the Codex session
contract. See the [usage guide](./docs/README.md).

## Docs

- [CLI skill](./skills/issue-finder-cli/SKILL.md)
- [Reference](./docs/README.md)
- [Repository guide](./AGENTS.md)

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

For isolated CLI runs, set `ISSUE_FINDER_HOME=/tmp/issue-finder-demo`.
The repository is licensed under the [MIT License](./LICENSE).
