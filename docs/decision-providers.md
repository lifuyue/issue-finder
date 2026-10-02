# Decision model providers

Issue Finder defaults to Alibaba `decision-model-preview`. Cloudflare
`clef-flash` is a second native decision provider; Codex CLI remains an explicit
fallback. All three answer the same seven `scout-semantics-v2` questions from
the same bounded materials. They do not change ranking policy or repair
authorization. `assess` makes no model request.

## Choose and configure a provider

Configuration lives in `config.toml` under the resolved Issue Finder state
directory (`~/.issue-finder`, overridden by `ISSUE_FINDER_HOME`). API credentials
are read only from the named environment variables. Keep their values out of
configuration files, chat, arguments, and logs.

| Configuration provider | Fixed model | Authentication | Endpoint/account |
| --- | --- | --- | --- |
| `aliyun_decision` (default) | `decision-model-preview` | `DASHSCOPE_API_KEY` | Complete Workspace endpoint in `ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT` or `[decision.aliyun_decision].endpoint` |
| `cloudflare_clef_flash` | `clef-flash` | `CLOUDFLARE_API_TOKEN` | `CLOUDFLARE_ACCOUNT_ID` or `[decision.cloudflare_clef_flash].account_id` |
| `codex` | `gpt-6-luna`, reasoning `none` | Existing CLI login or explicit `ISSUE_FINDER_CODEX_AUTH_JSON` bootstrap | CLI discovery, `[decision].codex_binary`, or `ISSUE_FINDER_CODEX_BIN` |

Existing configurations that omit `provider` now select Alibaba.
Set `provider = "codex"` explicitly to retain the earlier CLI deployment;
installing the CLI or
initializing its authentication alone does not select it for scout.

Alibaba configuration, with a placeholder to replace using your actual Workspace:

```toml
[decision]
provider = "aliyun_decision"
concurrency = 4
candidate_budget = 24
timeout_seconds = 45

[decision.aliyun_decision]
endpoint = "https://YOUR_WORKSPACE.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone"
```

The supported official regions are `cn-beijing` and `ap-southeast-1`. Supply the
complete `/compatible-mode/v1/systemone` URL, including your Workspace hostname;
the empty default endpoint is not usable. The runtime endpoint variable overrides
this setting. A generic chat-completions endpoint is a different API.

Cloudflare configuration:

```toml
[decision]
provider = "cloudflare_clef_flash"
concurrency = 4
candidate_budget = 24
timeout_seconds = 45

[decision.cloudflare_clef_flash]
account_id = "YOUR_ACCOUNT_ID"
```

The runtime account variable overrides the configured ID. The adapter generates
`https://api.cloudflare.com/client/v4/accounts/{id}/ai/run/@cf/cloudflare/clef-flash`.
An optional `[decision.cloudflare_clef_flash].endpoint` override supports local
mock servers; ordinary production setup does not need it. Use Cloudflare's
[Workers AI REST API token template](https://developers.cloudflare.com/workers-ai/get-started/rest-api/)
for the intended account. The getting-started guide recommends Read and Edit;
the Run API lists Read or Write as accepted permissions. Verify the actual
request rather than inferring access from the existence of a token.

To use the Codex fallback for scout, explicitly set:

```toml
[decision]
provider = "codex"
concurrency = 4
candidate_budget = 24
timeout_seconds = 45
codex_binary = "" # Discover an existing compatible CLI.
```

For injected file authentication, run `issue-finder decision-auth-init` at task
runtime startup, then `issue-finder decision-check --provider codex`. Initialization
preserves an existing refreshed file; it does not verify model access. Its
private home, refresh persistence, and seed rotation rules remain in the
[Codex auth guide](decision.md#injected-codex-authentication) and
[Cloud configuration handoff](cloud-codex-auth-prompt.md).

`decision-check` uses the configured provider. Diagnostic overrides are explicit:

```bash
issue-finder decision-check --provider aliyun-decision
issue-finder decision-check --provider cloudflare-clef-flash
issue-finder decision-check --provider codex
```

The configuration names use underscores; CLI choices use hyphens. A diagnostic
override does not change subsequent scout selection. `--codex-binary PATH`
implicitly selects Codex and conflicts with an explicitly selected native
provider. The Codex installation wrapper still checks Codex explicitly.
Missing credentials, unsupported models, HTTP errors, invalid answers, and
timeouts are reported for the selected provider. The program never silently
switches providers, substitutes semantic keyword rules, or enables a paid plan.

## Native requests and results

Alibaba and Cloudflare use native System One `state` and `questions`, with
typed `choice`, `noul`, and `score` questions. The adapter maps Boolean to
`noul`, Choice to an option-description map, and ordered Score to levels in
ascending order. Instructions retain the business question's prompt and
criteria. Each original context is identified under `state.question_materials`
by question ID; each typed question selects its own material. One request per
issue carries all seven questions. This preserves material scopes and introduces
no shared-input prompt experiment or ordinary chat JSON generation.

Answers must match requested IDs, types, and allowed values. Native Boolean
results supply `P(yes)`; Choice results supply their option distribution; Score
results supply the ordered distribution and its expected level, which may lie
between levels. Probability data is retained on the answer. Original native
answers, including service `confidence` and Score `legend`, remain under
`metadata.native_answers`. Confidence is not a replacement for probability,
and probability does not establish that unavailable evidence exists. The Codex
fallback has no verified native probabilities; its probability fields remain
absent. The adapter does not fabricate one-hot distributions or sample repeated
generations to estimate probabilities.

Cloudflare responses wrap the model output in `result`; the adapter checks the
outer `success` and errors before validating model answers. Alibaba can report
`request_id` and `latency_ms`. Metadata separates a reported
`server_latency_ms` from client-observed request duration; Cloudflare's documented
output does not require a server latency. Usage is preserved when returned.
Provider fingerprints include endpoint, model, and input adapter identity so
cached decisions are not shared across incompatible provider configurations.
The six-hour semantic cache still never replaces fresh GitHub availability facts.

## Published costs and limits

Official documentation was reviewed on 2026-10-02; account-specific entitlement
and remaining quota have not been verified.

| Provider | Published pricing or allowance | Practical limit |
| --- | --- | --- |
| Alibaba decision preview | Limited-time free in Beijing and Singapore; no public end date in the reviewed model page | Not a permanent-free commitment; current model page lists 1,200 RPM and 2,000,000 TPM |
| Cloudflare Clef-flash | $0.09 per million input tokens; 8,182 Neurons per million input tokens | Workers AI shares 10,000 free Neurons/day across models, resetting at 00:00 UTC |
| Codex CLI | Uses the selected CLI account's plan and limits | No new free allowance is established by reusing an existing login |

Cloudflare Free requests beyond the allowance fail; Workers Paid overage is
$0.011 per 1,000 Neurons. The Workers Paid account minimum of $5/month is a
subscription fee, not evidence of $5 gifted inference credits. This investigation
has not checked the user's personal credits, consumption, or billing plan.
Alibaba's limited-time offer likewise does not prove the actual Workspace can
call the model. Verify current console terms before changing billing settings.

Concurrency defaults to four issues and the candidate budget to 24; the timeout
is 45 seconds. Increasing budgets increases possible requests and input volume.
One successful minimal check proves that request's runtime access, not batch
throughput, classification quality, or remaining account quota. This integration
does not add fine-tuning or a broad quality benchmark.

## Cloud rollout and acceptance

Use the supported environment configuration workflow to preserve existing
GitHub credentials, proxy, CA, PATH, and the two-tool session v2 contract.
For the default Alibaba path, configure protected runtime `DASHSCOPE_API_KEY`
and non-secret `ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT` with the real Workspace
URL. For a deliberately selected Cloudflare path, configure protected runtime
`CLOUDFLARE_API_TOKEN` and non-secret `CLOUDFLARE_ACCOUNT_ID`. Do not copy
credentials into a saved image or source checkout. Do not enable paid service
or select another provider merely to make a failed acceptance check pass.

Install the supporting source revision, inspect `decision-check --help`, and
select the same provider in `[decision].provider` that scout will use. Save and
publish the environment settings through its normal review workflow, then
verify a fresh task's binding and run that provider's real `decision-check`.
Report the source revision, installed binary, configured provider, effective
non-secret endpoint/account, redacted readiness, and actual request result.
Keep unavailable or unverified items explicit.

Authentication checks and model access are separate: Alibaba's model-list API
can succeed while System One returns `AccessDenied.Unpurchased`. Confirm model
entitlement for the selected Workspace and region before retrying. Cloudflare
token verification can report `active` without proving Workers AI access; an
empty account list does not supply the required Account ID. Obtain that ID from
the intended account's dashboard, then test the actual model endpoint.

After a minimal check succeeds, an opt-in transport smoke test exercises the
production adapter with seven questions per issue and at most five frozen
public GitHub snapshots. With credentials already injected into the process
environment, run:

```bash
cargo run --example decision_benchmark -- smoke \
  --provider aliyun-decision --report /tmp/issue-finder-native-aliyun.json --live
```

Use `--provider cloudflare-clef-flash` to test Cloudflare. To start with one
snapshot, add `--ids clap-rs_clap_5919`. The default selection uses four
concurrent slots; this manual smoke mode allows 120 seconds per call,
independently of the production 45-second default. The report includes failures,
client elapsed time, available server latency, native probabilities and
diagnostic differences from frozen labels. Its peak concurrency measures client
requests in flight, not simultaneous inference inside the service. This small
test does not establish classification quality, current GitHub availability or
a provider speed ranking. The explicit `--live` option enables model calls; the
example is separate from ordinary offline tests. See the
[manual benchmark helper](../examples/decision_benchmark.rs).

Code and mock protocol validation cannot be reported as real Alibaba or
Cloudflare service acceptance. Historical Codex model checks remain evidence
for their captured Codex configuration only.

## Live acceptance checkpoint — 2026-10-02

Temporary process-scoped credentials were used; no environment configuration or
billing settings were changed. Clef's installed CLI health check (then named
`system1-check`, now `decision-check`) returned a
complete native answer in 567 ms, with a probability distribution and zero output
tokens. A seven-question single-snapshot request then passed in 842 ms.

The subsequent five-snapshot production-adapter smoke test completed 5/5 logical
requests, with all 35 native answers passing strict contract validation. The
client had four requests in flight; the fifth entered at 535 ms after a slot
became free. Total batch wall time was 1,258 ms and individual request times were
533–992 ms. These are client-observed durations from one small run, not a server
latency guarantee or a measured speedup over Codex. No server latency was returned.

Transport acceptance is separate from decision quality. Five of the 35 answers
differed from frozen labels across four issues. In particular, `clap-rs/clap#5919`
was classified as `open_discussion` instead of `concrete_change`, and the existing
policy hid this expected-visible fixture. The same discrepancy appeared in the
single-snapshot request. Other differences concerned maintainer encouragement,
preference matching and verification clues. This sample does not establish model
quality; prompts, ranking policy and default provider were not changed to fit it.
Frozen-material results do not establish current issue availability.

Alibaba remains blocked: the earlier actual request returned HTTP 403
`AccessDenied.Unpurchased`. The user subsequently confirmed the exact endpoint
and model in the Beijing console, where `INSUFFICIENT_AVAILABLE_QUOTA` indicated
available account quota below zero. No retry or recharge was performed after
that finding. Resolve the account quota restriction before retesting; if the API
still denies access, confirm Preview entitlement with Alibaba support. The console
finding does not by itself prove the sole cause of the earlier API error.

## Four versus eight concurrent Clef calls — 2026-10-02

The bounded experiment used the production adapter and 45-second call timeout,
with the same 24-call workload in each run and concurrency order `4, 8, 8, 4`.
The workload cycles five frozen issue snapshots; it is not 24 distinct issues.
Each issue still sends all seven questions in one request. Application caches
were bypassed, fresh HTTP clients were created per run, and remote caching was
uncontrolled. Request hashes confirm identical material and order across runs.

| Concurrency | Each 24-call run | Mean batch time | Call P50 / P95 across 48 calls | Final failures | Reported input tokens across 48 calls |
| --- | --- | --- | --- | --- | --- |
| 4 | 4.072 s, 2.581 s | 3.327 s | 423 / 903 ms | 0 | 186,432 |
| 8 | 1.912 s, 1.729 s | 1.821 s | 450 / 1,161 ms | 0 | 186,432 |

Eight concurrent calls reduced mean batch time by 45.3% in this run, while call
P95 increased by 28.6%. Eight is a reasonable opt-in for batch responsiveness;
four remains the unchanged program default. Two repetitions per setting and
repeated material do not establish production tail latency, service-side
parallelism or performance at higher concurrency. No quality tuning was done.
Final failures were zero; HTTP attempts, intermediate 429s and internal retries
were not separately instrumented, so this is not evidence of zero retries.

The 96 benchmark calls reported 372,864 input tokens and zero output tokens.
Two separate size probes reported 140 and 2,171 input tokens, bringing this
experiment's total to 375,175 inputs. Tokens are summed once per returned
request usage, not multiplied by seven questions. Every benchmark response
reported 3,884 inputs even though snapshot lengths differ; the adapter preserves
that service value. The size probes confirm usage responds to input length,
but do not explain this equality or reconcile it to account billing.

Using Cloudflare's published 8,182 Neurons per million Clef-flash input tokens:

- One issue at 3,884 inputs: approximately 31.78 Neurons.
- One full 24-candidate scout without cache hits: approximately 762.69 Neurons.
- A fresh 10,000-Neuron daily allowance: approximately 314 issues, or 13 complete
  24-candidate scout runs. Longer inputs and other Workers AI usage reduce this.
- This experiment, including size probes: approximately 3,069.68 Neurons, or
  30.7% of a fresh daily allowance. This is not a reading of remaining quota.

The allowance resets at 00:00 UTC (08:00 Asia/Shanghai) daily. At one full scout
per hour, this estimate covers about 13 active hours per day; at up to ten full
scouts per day, it fits within each renewed allowance, assuming no other usage.
It is a daily capacity, not a credit balance with a calculated expiration date.
The existing six-hour decision cache may avoid calls when materials, questions,
preferences and provider configuration still match; refresh or changed inputs
consume additional calls. Concurrency changes how fast work finishes, not its
reported token total in this experiment.

Evidence: [per-call benchmark report](evidence/clef-concurrency-2026-10-02.json)
and [input-size probes](evidence/clef-usage-probes-2026-10-02.json). These contain
no credentials. To repeat the bounded 96-call experiment with credentials already
injected, run:

```bash
cargo run --offline --example decision_benchmark -- concurrency \
  --provider cloudflare-clef-flash --report /tmp/issue-finder-clef-concurrency.json --live
```

Export the same 24-call workload and request-size manifest without model access:

```bash
cargo run --offline --example decision_benchmark -- workload \
  --report /tmp/issue-finder-decision-workload.json
```

The manual helper replaces the retired live-test environment controls. Neither
mode is part of the routine test suite; only `--live` authorizes paid requests.

## Official references

- [Alibaba model and pricing](https://help.aliyun.com/zh/model-studio/decision-model-preview)
- [Alibaba System One API](https://help.aliyun.com/zh/model-studio/decision-model-api)
- [Cloudflare Clef-flash model](https://developers.cloudflare.com/workers-ai/models/clef-flash/)
- [Cloudflare input schema](https://developers.cloudflare.com/workers-ai/models/clef-flash/schema-input.json)
- [Cloudflare output schema](https://developers.cloudflare.com/workers-ai/models/clef-flash/schema-output.json)
- [Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/)
- [Workers subscription pricing](https://developers.cloudflare.com/workers/platform/pricing/)
