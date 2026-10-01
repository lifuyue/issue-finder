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
`gh` is optional when another credential source is present. Python and MCP
registration are not prerequisites. Scout's System 1 requires an authenticated
Codex CLI, configured separately from the GitHub credential.

Missing configuration is valid. Defaults and per-call `profile` preferences are
sufficient; `issue-finder init` is optional and is not a hidden questionnaire.
Configuration, authentication, and network errors surface directly in business
calls, without a separate preflight tool.

Read access suffices for discovery and assessment. Codex handles authorized fork,
push, and PR delivery with its normal tools and the environment's credentials;
read access, successful local checks, or a pushed fork does not prove upstream
PR permission.

## Codex CLI: Cloud installation, local reuse

System 1 uses Codex app-server with `gpt-6-luna` and `reasoning.effort=none`.
The tested compatibility baseline is Codex CLI `0.159.3`. Version output or a
login-status message alone does not validate model access, authentication,
reasoning settings, or strict structured output.

After installing an Issue Finder build that exposes `system1-check`, local use
checks the existing Codex executable and login without installing or upgrading:

```bash
/path/to/issue-finder/scripts/system1-codex.sh --check-only
# If the selected binaries are outside PATH:
/path/to/issue-finder/scripts/system1-codex.sh --check-only \
  --codex-binary /absolute/path/to/codex \
  --issue-finder-binary /absolute/path/to/issue-finder
```

For an explicitly configured Cloud setup, install the pinned CLI into a private
npm prefix and immediately run the same live acceptance check:

```bash
/path/to/issue-finder/scripts/system1-codex.sh --cloud-install \
  --prefix /workspace/.issue-finder-runtime/codex-cli \
  --issue-finder-binary /absolute/path/to/issue-finder
```

Cloud installation requires Node.js/npm. It installs `@openai/codex@0.159.3`
with `npm --prefix`, checks the exact installed version, and does not change the
global npm install, shell profile, PATH, or Codex configuration. Supply that
prefix's `node_modules/.bin/codex` through `ISSUE_FINDER_CODEX_BIN` in the supported
Cloud runtime settings or `[system1].codex_binary`. Persist the setting in the
saved environment and verify a fresh task. The repository script does not
publish or edit Cloud settings. Do not set an existing Codex override during
`--cloud-install`; that mode explicitly chooses its private installed binary.

Both paths invoke the production `issue-finder system1-check --codex-binary PATH`
adapter. This makes a real minimal schema-constrained request in an independent
context with no classification tools and checks protocol-confirmed model and
reasoning settings plus typed answer validation. A successful request validates
runtime authentication. The command returns one redacted JSON object on stdout;
installation and diagnostic logs go to stderr. A failed check exits unsuccessfully
and must not be interpreted as a working environment. Existing newer local CLI
versions may be compatible; verify the actual request instead of automatically
installing the baseline version.

Local discovery supports `ISSUE_FINDER_CODEX_BIN`, configured
`[system1].codex_binary`, and the existing CLI's discoverable installation. It
reuses the user's available login and leaves global configuration alone.
The check-only script discovers PATH or an explicit executable override; use
`issue-finder system1-check` directly to exercise configured binary discovery.
A scout provider reuses one app-server process with independent threads per issue,
separate from the agent's working chat. Each issue sends all seven v2 questions in
one request while retaining each question's material and criteria. Default
`[system1].concurrency = 4` must be positive and has no additional upper cap;
`candidate_budget = 24` limits screened issues independently. Candidate failures
are isolated; the adapter permits at most one retry for a retryable server response
within the original timeout. A minimal `system1-check` validates runtime access;
it does not establish batch throughput or v2 classification quality. Historical
single-request and eight-question reports remain historical evidence.
Default settings and screening behavior are in the [System 1 guide](../../../docs/system1.md).

### Codex authentication

Provision Codex authentication through the host's supported credential mechanism;
`GH_TOKEN` only authenticates GitHub. The script neither initializes login nor
reads, prints, copies, or commits authentication tokens. Never pass credential
values in command arguments, issue arguments, chat, logs, or repository files.

If Cloud bootstrapping injects a Codex-managed `auth.json`, use a private writable
`CODEX_HOME` distinct from the main agent's configuration, restrict file access,
and avoid replacing an existing user's auth/configuration. The adapter uses the
provisioned CLI authentication in its independent context. Do not treat a static
injected JSON file as permanent login: CLI refreshes change the runtime copy,
not the original injected value. Concurrent tasks initialized from the same
refresh credential can conflict. Without refresh persistence, this is a
transitional setup whose injected credential must be replaced after expiry.
Long-lived cross-task credential management is a separate deployment choice.
Acceptance requires a successful real model request, not file existence or
`codex login status` alone.

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
