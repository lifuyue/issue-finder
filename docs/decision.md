# Decision model screening

`scout` uses a bounded semantic decision step before final candidate selection.
GitHub retrieval, fresh initial availability checks, deduplication, and initial
ordering run first. The Decision model then answers seven fixed questions from issue material;
deterministic policy consumes the typed answers. Before display, scout rechecks
provisional recommendations with fresh GitHub evidence and backfills excluded entries
from the bounded candidate pool within the same API budget. Rechecking adds no candidate
discovery pages or model calls; its bounded PR searches use the existing API budget. This applies to
the default Codex session tool and direct `issue-finder scout` command. Legacy
control, dispatch, daily, and shared non-Codex workflows retain their compatibility
behavior.

The fixed question IDs are `task_type`, `description_quality`,
`contribution_signal`, `scope`, `preference_match`, `verification_clues`, and
`maintainer_signal`, versioned as `scout-semantics-v2`. They cover task content,
contribution statements, scope, and established preference matches. The Decision model
does not set an overall model score, authorize repairs, verify a claimed fix, or replace GitHub facts about
assignees, issue state, repository state, or linked PRs. Expressing interest is
not working on a fix; `not_observed` means no relevant statement was seen in the
provided material. Missing or sampled comments cannot establish that nobody is
working on the issue.

| Question | Allowed answers |
| --- | --- |
| `task_type` | `concrete_change`, `support_question`, `open_discussion`, `unclear` |
| `description_quality` | `clear`, `partial`, `unclear` |
| `contribution_signal` | `interest_only`, `working`, `withdrawn`, `fix_claimed`, `conflicting`, `not_observed`, `unknown` |
| `scope` | `bounded`, `design_needed`, `broad`, `unclear` |
| `preference_match` | `matches`, `mismatch`, `not_specified`, `unclear` |
| `verification_clues` | `present`, `absent`, `unclear` |
| `maintainer_signal` | `encouraged`, `clarification_needed`, `deferred`, `not_observed`, `unknown` |

An explicit unable-to-answer status leaves the typed answer absent. It is distinct
from selecting an allowed `unclear` or `unknown` answer and from provider failure.

## Naming and compatibility

The product calls this component the **Decision model**. Use `[decision]`,
`decision-check`, `decision-auth-init`, and `scripts/decision-codex.sh` for new
configuration and startup commands. The legacy `[system1]` configuration and
`system1-check` / `system1-auth-init` CLI aliases remain accepted, and
`scripts/system1-codex.sh` forwards to the new script. Existing auth, CLI and
cache directories under `~/.issue-finder/system1` or `cache/system1` retain their
paths so installed state stays usable. `ISSUE_FINDER_CODEX_*` names are unchanged.
The external providers' official **System One** protocol name and `/systemone`
URL are retained. Historical evidence records keep their original names and
commands; current reports expose `decision` and `diagnostics.decisionReplayPath`.

## Materials and independent context

The business layer builds only the context needed for each fixed question:
issue title/body, selected bounded discussion, comment author association and
time, repository context, and the caller's resolved preferences. The snapshot
records material scope, sampling, truncation, missing information, and input
identity. Titles are bounded to 512 characters and bodies to 12,000. Up to the
latest 30 comments are retained chronologically, with 4,000 characters per body
and 24,000 comment-body characters in total; newest comments are preserved when
that budget is exceeded. Comment scope reports complete, sampled, or unavailable
material plus omitted counts, truncation, and timeline completeness. GitHub text
is untrusted analysis data and cannot change instructions.
Prior keyword conclusions and ranking scores are not supplied as answers.

Native decision providers send one System One request per issue. Its `state`
maps each question's original context under `question_materials`; typed questions
carry their prompt and criteria and identify the matching materials. This retains
the seven existing material scopes. Native decisions do not use ordinary chat
completion with generated JSON, and this integration adds no shared-input prompt
experiment. See the [provider guide](decision-providers.md) for the wire contract,
configuration, probabilities, and costs.

The explicitly selected Codex fallback reuses one app-server process across concurrent
candidates. Each issue starts an independent thread and receives one request containing
all seven questions, retaining each question's criteria and relevant material. Threads
share no issue conversation or main-agent chat. The adapter reuses the installed CLI
and authentication. Classification has no shell, search, filesystem, or other tools. It requests
`gpt-6-luna` with `reasoning.effort=none`, uses strict JSON Schema, checks protocol
confirmation of the requested settings, and validates the response again in Rust.
A mismatch fails the call rather than switching model or reasoning level.

## Configuration and failures

Configuration files are optional, but the selected provider needs runtime
credentials and its endpoint or account. Defaults are independent of the legacy
`[llm]` configuration; enabling the Decision model does not enable that old review path:

```toml
[decision]
provider = "aliyun_decision" # Or cloudflare_clef_flash / codex; no automatic fallback.
concurrency = 4          # Positive number of in-flight candidates; no extra upper cap.
codex_binary = ""        # Discover an existing CLI when provider = "codex".
timeout_seconds = 45
candidate_budget = 24    # Hard cap per scout call; accepted range 0..100.
task_preferences = ""   # Explicit inclusion/exclusion preferences, if configured.

[decision.aliyun_decision]
endpoint = ""          # Complete Workspace HTTPS /compatible-mode/v1/systemone URL.

[decision.cloudflare_clef_flash]
account_id = ""        # Runtime CLOUDFLARE_ACCOUNT_ID may override.
```

Alibaba uses environment `DASHSCOPE_API_KEY` and fixed model
`decision-model-preview`; `ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT` overrides its
configured endpoint. Cloudflare uses environment `CLOUDFLARE_API_TOKEN`, fixed
model `clef-flash`, and the account's Workers AI run endpoint. Neither API key
belongs in `config.toml`. Missing configuration or authentication produces an
explicit failure; it never selects another provider or enables billing.
`decision-check` selects the configured provider, with explicit `--provider`
values `aliyun-decision`, `cloudflare-clef-flash`, and `codex`. A diagnostic
override does not change the configured scout provider.

For `[decision].provider = "codex"`, `ISSUE_FINDER_CODEX_BIN` selects an existing
executable. Cloud setup should
supply the private installed CLI path explicitly. Local execution only discovers
and reuses a CLI/login; it does not install, upgrade, or rewrite global Codex
configuration. A zero candidate budget explicitly skips semantic screening.
Concurrency limits in-flight candidates rather than questions; requests never split
one issue into seven calls. The candidate budget caps issues screened, independently
of concurrency. Fresh facts exclude known unavailable work before model screening.
The semantic pool includes candidates near the ranking boundary; task-form labels
and legacy keyword interpretations do not permanently exclude otherwise eligible work.

### Injected Codex authentication

This bootstrap is specific to the explicit Codex fallback. Set
`[decision].provider = "codex"` to use it for scout; native providers use their
own API keys and do not consume this auth home.

Cloud may deliver the complete raw Codex `auth.json` through the secret
`ISSUE_FINDER_CODEX_AUTH_JSON`; it is JSON text, not a file path or base64 value.
Use the supported secret environment-variable delivery so the program receives
the real JSON rather than a network-secret placeholder. Keep the value out of
arguments, repository files, chat, and logs. A normal CLI ChatGPT login cache can
refresh itself; externally managed `chatgptAuthTokens` caches are rejected by
the importer because they require an external refresh owner.

Each task's runtime startup must explicitly initialize the private writable auth
home. Never bake credentials into the environment snapshot:

```bash
issue-finder decision-auth-init
issue-finder decision-check --provider codex
```

The optional non-secret `ISSUE_FINDER_CODEX_HOME` selects that home. By default
it is `system1/codex-home` under the resolved Issue Finder state directory
(`~/.issue-finder`, or `ISSUE_FINDER_HOME`). Initialization validates the input
and creates a private directory and `auth.json` with Unix modes `0700` and
`0600`. It preserves an existing auth file; use
`issue-finder decision-auth-init --replace` only to deliberately reseed it while
no Decision model calls are active.
Codex `decision-check` and scout consume initialized auth rather than importing the
secret automatically. Injected auth with a missing runtime file fails instead
of falling back to the host login.

Only Codex child processes receive the selected `CODEX_HOME` and
`cli_auth_credentials_store="file"`; the main agent's home and environment are
unchanged. The adapter removes the raw secret and competing Codex/API auth
variables from those children, while preserving proxy and CA settings. Without
injected auth or a custom home, local execution keeps using the existing CLI
login. Admin authentication requirements can still restrict the allowed store,
login method, or workspace.

Codex refreshes update the writable runtime `auth.json`, not the static Cloud
secret. Keep the refreshed file between runs; do not overwrite it from the
original seed on each invocation. Recreated environments need a current seed,
and concurrent environments or jobs must not reuse one refresh credential.
Secure refresh write-back across environment lifetimes is a separate deployment
choice. See the [official CI/CD auth guidance](https://developers.openai.com/codex/auth/ci-cd-auth).
Successful initialization establishes local storage only. Runtime acceptance
requires a successful real `decision-check --provider codex` request;
file existence and login status alone do not establish model access.

### Responses and caching

Scout's `decision` output distinguishes completed answers, inability to answer,
provider failure, and budget-skipped candidates. Inspect each candidate's status,
`inputId`, `materials`, and `snapshotPath`, together with scout diagnostics.
Authentication failure, unavailable model, timeout, and invalid JSON or answer
values produce explicit partial results; they are not negative issue judgments.
A single candidate failure or cancellation is isolated from other candidates.
The Codex adapter retries at most once for an explicitly rejected overload RPC or structured
recoverable connection error, respecting retry timing plus random jitter within the
original timeout. Accepted-turn failures are conservatively reported without replay.
Lost responses trigger bounded status/interrupt cleanup, never an unconfirmed duplicate
submission. Process exit fails pending requests and is disclosed.
Diagnostics record concurrency, peak in-flight candidates, per-candidate queue and
execution time, model duration when available, and cache hits.
There is no fallback to the replaced semantic keyword rules. Logs go to stderr;
tool stdout remains a single JSON object.

Valid decisions can be reused for the same material, question definitions,
resolved preferences, and provider configuration. The provider fingerprint
includes endpoint, model, and input adapter identity; Codex also records its CLI
and reasoning settings. Changes in any of those inputs invalidate reuse.
The decision cache has a six-hour freshness
limit; `refresh: true` bypasses reuse. This caches semantic interpretations only:
initial and final availability checks still fetch fresh GitHub facts on every scout
call. A semantic cache hit cannot establish that work remains available.

Full scout ranking replay is recorded separately from reusable model decisions.
`diagnostics.decisionReplayPath` identifies a snapshot under
`cache/system1/replay` in the Issue Finder state directory. It preserves numeric
GitHub facts, typed judgments, resolved profile, and filtered feedback used by
the Codex ranking policy. Offline replay consumes those saved inputs and does
not depend on a live GitHub request, model request, or mutable live cache. Schema 2
captures explicitly name the v2 question set. Original schema 1 captures, with eight
v1 questions including `task_shape`, validate their original contract and preserve
captured scores, visibility and order; they are historical outcomes and are never
recomputed under v2 or accepted as current cache entries.

`assess` retrieves fresh issue/discussion material and runs the same final-depth
availability checks, including bounded linked-PR and PR-search evidence. It makes no
Decision model request and reports semantic screening as not evaluated for the new context.
A scout decision remains historical screening about its own snapshot. Both the session
tool and direct `issue-finder assess` command use the Codex facts path. Read further
comment pages and current code when they matter to the choice. The main agent owns
feasibility, reproduction, repair, verification and delivery.

## Availability evidence and selection policy

Availability is separate from semantic classification. The `availability` snapshot
records `checked_at`, issue state, assignees, archive/lock status, default branch,
linked/search coverage, actual PR identities and uncertainties. PR entries preserve
repository, number, API-verified state, draft status, `merged`/`merged_at`, base branch,
relation and sources. Closed PRs are not assumed merged. Cross-repository numbers
refer to their own repository. Only explicit resolving directives targeting this
issue establish a resolution relationship; mere mentions and search matches are leads.
Verified open resolution PRs establish strong competition. A verified merged resolution
PR targeting this repository's default branch warrants checking current code; it does
not prove reproduction or a fix. Branch-specific and cross-repository merges retain
that scope and require investigation before a new implementation. Missing, paginated, failed or unresolved search evidence remains unknown,
not proof that nobody is working or no fix exists.

Scout excludes closed, assigned, archived or locked work and strong open/merged
resolution evidence from default new-fix recommendations before semantic screening,
then refreshes provisional candidates and backfills within the bounded pool. Assess
still exposes factual context and actionable uncertainty when the issue is assessable.
Diagnostic candidates are investigation leads. `includeFiltered` can expose exclusions
and their reasons; it does not establish availability.

Concrete changes with clear goals, bounded scope and verification clues gain priority.
Documentation, content, generated, exercise, event and rewarded changes are governed
by their requested change, clarity, scope and explicit preferences. Task form has no
hard visibility rule or risk penalty. An empty template remains uncertain for triage.
Explicit preference mismatches can still exclude a task.

`contribution_signal` describes discussion expressions only. Interest and completed
withdrawals incur no competition penalty. `working`, `fix_claimed` and `conflicting`
provide a soft scoping reminder to check current evidence; they do not establish
assignment, an open PR or a merged fix, and cause no automatic contested category or
large competition penalty. Unknown answers remain visible with missing evidence.
Actual GitHub facts remain authoritative. The semantic path ignores old comment
counters, keyword risk tags and contribution-memory text hints; it never charges the
same conclusion again as a quality penalty. Compatibility workflows retain their
existing behavior.

## Replaceable provider boundary

The business contract contains candidate/input identity and fixed questions with
IDs, criteria, context, and Boolean, Choice, or ordered Score answer types.
Responses preserve question IDs and distinguish answered, unable-to-answer, and
provider-failed states. The adapter cannot redefine criteria or ranking weights.
A replacement provider implements this same contract; GitHub material assembly
and result policy do not depend on Codex protocol fields.

Optional probabilities come from the provider's native service. Boolean
probability, mutually exclusive Choice distributions, and ordered Score
expectations have distinct meanings. Alibaba and Cloudflare retain native
probabilities on answers and keep original confidence/legend information under
`metadata.native_answers`. A reported `server_latency_ms` is separate from client
request duration. The Codex fallback provides no verified probabilities, so its
probability information remains absent. Generated
percentages, fabricated one-hot distributions, and repeated sampling are not
substitutes for provider probabilities.

See [installation and live verification](../skills/issue-finder-cli/references/install.md)
for the Cloud/local setup paths. Offline regressions must not depend on a live
model; `issue-finder decision-check` is a separate real-request acceptance check.
Code and mock protocol validation do not establish successful real Alibaba or
Cloudflare access. The environment must provide that provider's credentials and
endpoint/account before live acceptance can be reported.
The [evaluation report](decision-evaluation.md) records historical v1 regression and
real-model checks, including the original 38/40 result. Those eight-question results
are preserved as historical evidence and do not establish v2 or concurrency acceptance.
Current offline fixtures are documented in the
[fixture guide](../tests/fixtures/decision_eval/README.md).
