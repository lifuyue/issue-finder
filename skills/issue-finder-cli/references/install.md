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

Session tools resolve nonempty `GH_TOKEN`, optional `[github].token`, then a bounded,
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

## Standard Cloud credential setup

Use one key, `GH_TOKEN`, for GitHub credentials. Issue Finder no longer reads
`GITHUB_TOKEN`, including through its `gh` fallback. If the previous environment
provided only `GITHUB_TOKEN`, configure the same credential as `GH_TOKEN` in the
Cloud settings and publish that change. No duplicate token or alias is needed.

Choose one delivery method for `GH_TOKEN`:

- **Environment variables** provides the actual value directly to programs. This
  is suitable when you control the environment and want direct credential
  delivery. Processes in that environment can read the value.
- **Network secrets** provides a placeholder. The proxy substitutes the real
  value for allowed HTTPS destinations on port 443, during setup and tasks.
  Configure `api.github.com` for API calls and `github.com` when needed.

Do not configure the same key in both places. These are delivery choices for
one credential, not two authentication identities. See
[OpenAI's Cloud environment documentation](https://learn.chatgpt.com/docs/environments/cloud-environments).
In either mode, the program reads `GH_TOKEN` and sends its value in the normal
Authorization header. Preserve the configured proxy and CA trust. Do not copy
the value into `config.toml`, `.env`, shell profiles, logs, or command arguments.

Save and publish the environment, then verify a new task: confirm `GH_TOKEN` is
present without printing it, and run a bounded `assess` with the installed
binary. Inspect `success`, `partial`, and missing-evidence warnings. Existing
tasks may have different bindings from the published settings. If the key is
absent, check the task's environment/version and any Personal vault scope.

Authentication success identifies the effective GitHub account, not which raw
secret the proxy used or which other endpoints it permits. Diagnose permission
errors from the actual response; changing the local variable name does not
change GitHub permissions.
