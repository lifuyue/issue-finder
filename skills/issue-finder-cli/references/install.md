# CLI installation and compatibility

Install the binary in the environment where the current agent executes commands.
A CLI installed on the user's laptop is not automatically available in a remote
or cloud project. Git is required. Python and MCP are not required for this skill.

The official release channel is
[lifuyue/issue-finder releases](https://github.com/lifuyue/issue-finder/releases/latest).
Use its latest stable release, not an arbitrary fork or an unpinned development
branch. The release archive includes platform binaries and `SHA256SUMS`; current
build targets are Apple Silicon macOS, x86-64 Linux GNU, and x86-64 Windows MSVC.
Download the matching archive, verify its SHA-256 against the release checksums,
and install its binary in an authorized directory on PATH. Other platforms can
build the crate using Rust 1.89+ and Cargo.

With Cargo available, the published package install/update command is:

```bash
cargo install issue-finder --locked
```

Do not claim that a published version supports this skill until checking:

```bash
issue-finder --version
issue-finder tools --profile session list
```

This skill requires `sessionContractVersion: 1` and the seven tools listed in
`SKILL.md`. Matching version numbers do not replace this capability check. If an
older binary remains first on PATH, locate it and explain the path mismatch.
Do not repeatedly reinstall the same release when it lacks the contract.

For development before a compatible stable package is published, the user can
explicitly install this checked-out project:

```bash
cargo install --path /absolute/path/to/issue-finder --locked
```

This is a source installation, not a claim about the latest public release. Run
it only when installation from the current checkout is authorized. This skill
never installs or updates itself, the binary, or target dependencies silently.

## Cloud environment persistence

`cargo build` only creates a build artifact; it does not install `issue-finder`
on PATH. An environment installation step must install the binary, and its
startup configuration must make that installed location available to subsequent
agent commands. Exporting PATH inside a one-off child shell is not persistent
environment configuration. For an authorized source install with a custom
`CARGO_HOME`, install into that home and persist its `bin` directory in PATH.
Use the environment's configured Rust version; do not hardcode one from a past task.

Verify in a fresh command shell: `command -v issue-finder`, `issue-finder --version`,
and `issue-finder tools --profile session list`. A successful invocation by an
absolute `target/debug` path does not satisfy this check. After changing the
environment template, use its supported publication workflow and verify a new
task created from it. Repairing the current VM or a local setup script alone does
not update the saved template or already-running tasks. If no configuration-write
tool is available, prepare the exact setup changes and report that publication
remains pending.

Preserve the host's proxy and CA configuration. Keep TLS certificate verification
enabled. Some tools, including Maven, need explicit proxy configuration even when
curl works; use a task-local configuration derived from the supported environment.
Put caches in writable locations when a default home cache is read-only. Treat
network, TLS, cache, and GitHub authorization failures as separate diagnoses.

## GitHub authentication

For GitHub authentication, session tools check `GITHUB_TOKEN`, then
`[github].token` in optional configuration, then captured `gh auth token
--hostname github.com`. Existing credentials are never printed or persisted by
the fallback. If none work, have the user authenticate with `gh auth login` in
the execution environment, or configure a read-capable token through the host's
secret mechanism. For private Git clone/fetch, Git credentials must also work;
`gh auth setup-git` is the GitHub CLI setup option. Do not ask the user to paste
tokens into the conversation.

Read access is sufficient for discovery, not for the whole contribution workflow.
For fork/push/PR delivery, use the separate capability checks and credential
guidance in [GitHub delivery](github-delivery.md). CLI + skill does not inherently
need an extra authorization round when the host already supplies a working identity.

Default configuration is enough to begin. Preferences can be passed in each
tool call. `issue-finder init` remains optional and interactive; do not drive it
as a hidden questionnaire in the agent workflow.
