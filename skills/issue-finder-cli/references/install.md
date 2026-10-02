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
registration are not prerequisites. Scout's Decision model requires credentials for
its selected provider, separately from the GitHub credential. The default is
Alibaba `decision-model-preview`; Cloudflare `clef-flash` and Codex CLI are
explicit alternatives. Codex CLI is required only for the Codex fallback.

Missing configuration is valid. Defaults and per-call `profile` preferences are
sufficient; `issue-finder init` is optional and is not a hidden questionnaire.
Configuration, authentication, and network errors surface directly in business
calls, without a separate preflight tool.

Read access suffices for discovery and assessment. Codex handles authorized fork,
push, and PR delivery with its normal tools and the environment's credentials;
read access, successful local checks, or a pushed fork does not prove upstream
PR permission.

## Native decision providers

Choose `[decision].provider = "aliyun_decision"` (default) or
`"cloudflare_clef_flash"` in the Issue Finder configuration. `decision-check`
checks that configured choice; a `--provider` diagnostic override does not change
scout configuration. Native keys are read only from their named runtime
environment variables; do not write credentials into `config.toml`.

| Provider | Secret environment key | Non-secret runtime input | Explicit acceptance |
| --- | --- | --- | --- |
| Alibaba | `DASHSCOPE_API_KEY` | `ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT`, or `[decision.aliyun_decision].endpoint` | `issue-finder decision-check --provider aliyun-decision` |
| Cloudflare | `CLOUDFLARE_API_TOKEN` | `CLOUDFLARE_ACCOUNT_ID`, or `[decision.cloudflare_clef_flash].account_id` | `issue-finder decision-check --provider cloudflare-clef-flash` |

Alibaba needs the complete Workspace URL
`https://{workspace}.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone`
or its `ap-southeast-1` equivalent. A generic DashScope chat URL does not provide
this decision API. Cloudflare generates the Workers AI endpoint from the account
ID. Prefer its Workers AI REST API token template and verify access on the actual
account. Preserve the host's proxy and CA settings. Provider failures remain
visible; the program does not silently switch providers or activate paid plans.

The [provider guide](../../../docs/decision-providers.md) includes full
configuration, native probabilities, published costs, and live acceptance limits.
Alibaba's current limited-time free offer has no published end date; Cloudflare's
shared 10,000 Neurons/day allowance is not evidence of this user's unused quota.
The Workers Paid $5 monthly minimum is a subscription fee, not gifted credits.
Offline/mock checks do not validate either user's real service access.

## Codex CLI fallback: Cloud installation, local reuse

With `[decision].provider = "codex"`, Decision model uses Codex app-server with
`gpt-6-luna` and `reasoning.effort=none`.
The tested compatibility baseline is Codex CLI `0.159.3`. Version output or a
login-status message alone does not validate model access, authentication,
reasoning settings, or strict structured output.

After installing an Issue Finder build that exposes `decision-check`, local use
checks the existing Codex executable and login without installing or upgrading:

```bash
/path/to/issue-finder/scripts/decision-codex.sh --check-only
# If the selected binaries are outside PATH:
/path/to/issue-finder/scripts/decision-codex.sh --check-only \
  --codex-binary /absolute/path/to/codex \
  --issue-finder-binary /absolute/path/to/issue-finder
```

For an explicitly configured Cloud setup, install the pinned CLI into a private
npm prefix and immediately run the same live acceptance check:

```bash
/path/to/issue-finder/scripts/decision-codex.sh --cloud-install \
  --prefix /workspace/.issue-finder-runtime/codex-cli \
  --issue-finder-binary /absolute/path/to/issue-finder
```

Cloud installation requires Node.js/npm. It installs `@openai/codex@0.159.3`
with `npm --prefix`, checks the exact installed version, and does not change the
global npm install, shell profile, PATH, or Codex configuration. Supply that
prefix's `node_modules/.bin/codex` through `ISSUE_FINDER_CODEX_BIN` in the supported
Cloud runtime settings or `[decision].codex_binary`. Persist the setting in the
saved environment and verify a fresh task. The repository script does not
publish or edit Cloud settings. Do not set an existing Codex override during
`--cloud-install`; that mode explicitly chooses its private installed binary.

`--codex-binary` implies Codex selection and conflicts with an explicitly chosen
native `--provider`. Both script paths invoke the production
`issue-finder decision-check --codex-binary PATH` adapter. This makes a real
minimal schema-constrained request in an independent
context with no classification tools and checks protocol-confirmed model and
reasoning settings plus typed answer validation. A successful request validates
runtime authentication. The command returns one redacted JSON object on stdout;
installation and diagnostic logs go to stderr. A failed check exits unsuccessfully
and must not be interpreted as a working environment. Existing newer local CLI
versions may be compatible; verify the actual request instead of automatically
installing the baseline version.

Local discovery supports `ISSUE_FINDER_CODEX_BIN`, configured
`[decision].codex_binary`, and the existing CLI's discoverable installation. It
reuses the user's available login and leaves global configuration alone.
The check-only script discovers PATH or an explicit executable override; use
`issue-finder decision-check --provider codex` directly to exercise configured
binary discovery. The Codex scout provider reuses one app-server process
with independent threads per issue,
separate from the agent's working chat. Each issue sends all seven v2 questions in
one request while retaining each question's material and criteria. Default
`[decision].concurrency = 4` must be positive and has no additional upper cap;
`candidate_budget = 24` limits screened issues independently. Candidate failures
are isolated; the adapter permits at most one retry for a retryable server response
within the original timeout. A minimal `decision-check` validates runtime access;
it does not establish batch throughput or v2 classification quality. Historical
single-request and eight-question reports remain historical evidence.
Default settings and screening behavior are in the [Decision model guide](../../../docs/decision.md).

### Codex authentication

`decision-auth-init` initializes the explicit Codex fallback only. Set the scout
choice explicitly; this configuration is separate from a one-off diagnostic:

```toml
[decision]
provider = "codex"
```

Native Alibaba and Cloudflare providers do not need this auth initialization.

`GH_TOKEN` only authenticates GitHub. By default the script reuses an existing
Codex login. For explicit Cloud injection, store the complete original `auth.json`
as `ISSUE_FINDER_CODEX_AUTH_JSON` in the host's protected **environment-variable**
settings, available during task execution. Supply real JSON, not a file path,
base64, or a Network secret placeholder. Never put the value in arguments, chat,
logs, repository files, or a saved environment image.

After installing a source revision supporting the new command, initialize at
each task's runtime startup, then verify the real provider:

```bash
issue-finder decision-auth-init
issue-finder decision-check --provider codex
# Or use the repository wrapper (also supports --cloud-install):
/path/to/issue-finder/scripts/decision-codex.sh --check-only --auth-from-env
```

The optional non-secret `ISSUE_FINDER_CODEX_HOME` chooses an absolute private,
writable directory; the default is `system1/codex-home` under `ISSUE_FINDER_HOME`
or `~/.issue-finder`. Bootstrap validates the credential shape, creates the Unix
directory with mode `0700` and `auth.json` with mode `0600`, and preserves existing
refreshed credentials. `--replace` is an explicit seed rotation operation to use
while no Decision model calls are active, never a default startup flag. External
`chatgptAuthTokens` caches require their own refresh owner and are not imported.

The adapter sets `CODEX_HOME` and the file credential store only on its Codex
children, removes the raw injection and competing auth environment variables
there, and leaves the main agent login, proxy, and CA configuration intact.
Opting into injection with a missing auth file fails with initialization guidance;
runtime business calls never silently bootstrap or fall back to another login.

CLI refresh updates the runtime file, not the static injected secret. Preserve
that refreshed file between calls. Recreated environments need a current seed,
and separate concurrent tasks must not reuse one rotating refresh credential.
Without secure refresh persistence, this is a transitional setup whose injected
credential must be replaced after expiry or rotation. See the
[official CI/CD auth guidance](https://developers.openai.com/codex/auth/ci-cd-auth).
Successful bootstrap only reports local storage; acceptance requires a successful
real model request, not file existence or `codex login status` alone. A ready-to-use
[Cloud configuration task prompt](../../../docs/cloud-codex-auth-prompt.md) describes
the rollout and fresh-task checks.

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
