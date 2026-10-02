# Issue Finder reference

Issue Finder is a Rust CLI that supplies GitHub issue discovery and assessment
to Codex. Codex selects issues, prepares workspaces, reproduces and fixes bugs,
validates changes, and delivers authorized PRs.

## Entry points

The installable skill is [issue-finder-cli](../skills/issue-finder-cli/SKILL.md).
The [root README](../README.md) covers installation and invocation;
the [environment guide](../skills/issue-finder-cli/references/install.md) covers
Cloud setup and credentials.

`tools` and `mcp` default to the `session` profile. Contract version 2 exposes
two business tools:

| Tool | Result |
| --- | --- |
| `issue-finder.scout` | Ranked candidates, seven v2 Decision model answers, fresh availability facts, material snapshots, diagnostics and budgets |
| `issue-finder.assess` | Issue body, one discussion page, fresh final-depth availability and repository evidence, warnings; no model request |

```bash
issue-finder tools list
issue-finder tools call issue-finder.scout --arguments '{"repo":"owner/repo","limit":5}'
issue-finder tools call issue-finder.assess --arguments '{"issue":"owner/repo#123"}'
```

The installed catalog defines argument bounds and defaults. Calls return a JSON
envelope with `success`, `status`, and `structured_content`. Parameter and output
semantics are documented in the [tool reference](../skills/issue-finder-cli/references/tools.md).

The [Decision model guide](decision.md) describes finite semantic questions, the
provider boundary, default concurrency 4, material-scoped
caching, failure isolation and runtime verification. One request per issue carries
all seven questions and their own material. Concurrency must be positive, with no
additional upper cap; the Codex adapter retries at most once for a retryable server
response within its original timeout.

The [decision provider guide](decision-providers.md) covers the default Alibaba
`decision-model-preview`, Cloudflare `clef-flash`, and explicit Codex fallback.
Configure `[decision].provider` as `aliyun_decision`, `cloudflare_clef_flash`, or
`codex`. Native providers use typed System One requests and service probabilities;
they do not require Codex CLI authentication. No failure silently changes providers.
`decision-check` checks the configured provider; `--provider aliyun-decision`,
`--provider cloudflare-clef-flash`, or `--provider codex` selects a diagnostic.

For the Codex fallback, Cloud can bind a complete raw `auth.json` as the secret
`ISSUE_FINDER_CODEX_AUTH_JSON`. Each task's runtime startup explicitly runs
`issue-finder decision-auth-init` before `decision-check --provider codex`;
runtime calls consume initialized credentials without writing the seed.
`ISSUE_FINDER_CODEX_HOME` optionally selects a private writable home used only
by Codex child processes. Initialization preserves existing refreshed auth unless
`--replace` is requested. Static secrets do not receive token refreshes; preserve
the runtime copy and avoid sharing one refresh credential across concurrent
environments. See [authentication limits](decision.md#injected-codex-authentication)
and the [Cloud configuration task prompt](cloud-codex-auth-prompt.md).

## Evidence and state

An explicit `repo` limits discovery to that repository. `search` selects bounded
GitHub search; omitting it uses the curated feed. Per-call `profile` values
override configured preferences. Ranking scores express recommendation factors,
not authorization to repair an issue.

Assessment reads the requested issue and discussion page on each call.
`refresh: true` also refreshes cached assessment evidence. Scout screening is
scoped to its material snapshot; assess does not reuse it as a fresh fact. Scout
checks fresh availability before semantic screening and refreshes provisional results
before display, backfilling within the bounded pool and existing API budget. A
six-hour semantic cache hit never replaces fresh GitHub facts. Discussion claims
provide soft reminders; concrete task forms do not incur automatic exclusions.
Actual PR evidence distinguishes repository identity, open/closed state, merges,
base branch and explicit resolution relationships. Mentions, search leads and
incomplete coverage remain uncertain. Pagination, warnings,
and `partial` results describe evidence limits; no matching PR in the returned
evidence does not establish that no competing PR exists. Original schema 1
replays with eight v1 questions retain captured scores, visibility and order after
original-contract validation. Schema 2 identifies current v2; old reports do not
establish v2 acceptance.

State defaults to `~/.issue-finder`; `ISSUE_FINDER_HOME` overrides that directory.
Configuration is optional. `GH_TOKEN` is the only GitHub credential environment key
read by Issue Finder; `GITHUB_TOKEN` is ignored. Session tools retain configured
`[github].token` and the host's stored `gh` login as compatibility fallbacks.
Configuration, authentication, and network errors are returned by business calls.

The session tools can record shown/read events for ranking. They ignore
`dismissed`, `done`, and `prepared` feedback and do not maintain workspace tasks,
execute project checks, or record PR completion.

## Other implemented interfaces

The CLI also contains terminal commands, handoff/context generation, inbox,
dispatch, contribution memory, and evaluation commands. Separate `control` and
`worker` tool profiles support those interfaces. They are outside the default
Codex session contract. Handoff output contains `codex.md` and context files;
it does not generate a Skill. `issue-finder --help` and subcommand `--help`
describe available commands and options.

## Development

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Offline evaluation inputs live in `tests/fixtures/`. Recommendation reports can
be generated with `issue-finder eval recommendation --offline --output <dir>`.
CI runs Rust format, lint, and test checks on macOS. Repository conventions are
in [AGENTS.md](../AGENTS.md).
