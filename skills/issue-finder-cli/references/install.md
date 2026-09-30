# Environment installation and compatibility

Cloud environment configuration owns CLI installation, dependencies, PATH, and
supported credentials before Codex executes the task. A desktop installation
does not supply a cloud runtime. Environment startup does not guarantee that
runtime authentication or network access works; `scout` and `assess` return the
actual failures when called. A missing CLI cannot run its own readiness check.

## Install and persist

The official release channel is
[lifuyue/issue-finder releases](https://github.com/lifuyue/issue-finder/releases/latest).
Platform archives include binaries and `SHA256SUMS`; verify the matching archive
before installing its binary on PATH. Supported release targets are Apple Silicon
macOS, x86-64 Linux GNU, and x86-64 Windows MSVC. With Rust 1.89+ and Cargo:

```bash
cargo install issue-finder --locked
```

For an authorized installation from this checkout before a compatible release:

```bash
cargo install --path /absolute/path/to/issue-finder --locked
```

This is a source installation, not evidence that the published package supports
the current contract. Do not repeatedly install a release that lacks support.

`cargo build` creates an artifact, not a PATH installation. Configure the installed
binary's directory in the environment startup PATH; a one-off child-shell export
is not persistent configuration. Verify from a fresh command shell:

```bash
command -v issue-finder
issue-finder --version
issue-finder tools list
```

Require `sessionContractVersion: 2` with exactly `issue-finder.scout` and
`issue-finder.assess`. Package version alone does not establish compatibility.
The default catalog and MCP profile are `session`; `--profile session` is an
explicit equivalent. Updating the current VM does not update a saved cloud
template or already-running tasks; use the environment's supported configuration
workflow and verify a new task when changing that template.

Preserve the host's proxy and CA configuration, keep TLS verification enabled,
and use writable cache paths. Setup and Codex can use different shells or network
settings; diagnose runtime failures from the actual call.

## Credentials and optional configuration

Session tools resolve `GITHUB_TOKEN`, optional `[github].token`, then a bounded,
captured `gh auth token --hostname github.com` lookup. The fallback does not print
or save the credential. Configure read access through the host's supported secret
or authentication mechanism; never put tokens in JSON arguments or chat.
`gh` is optional when another credential source is present. Python, MCP
registration, a model API key, and a separate Codex CLI are not prerequisites.

Missing configuration is valid. Defaults and per-call `profile` preferences are
sufficient; `issue-finder init` is optional and is not a hidden questionnaire.
Configuration, authentication, and network errors surface directly in business
calls, without a separate preflight tool.

Read access suffices for discovery and assessment. Codex handles authorized fork,
push, and PR delivery with its normal tools and the environment's credentials;
read access, successful local checks, or a pushed fork does not prove upstream
PR permission.
