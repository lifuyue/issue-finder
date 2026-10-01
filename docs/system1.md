# System 1 screening

`scout` uses a bounded semantic decision step before final candidate selection.
GitHub retrieval, fresh initial availability checks, deduplication, and initial
ordering run first. System 1 then answers seven fixed questions from issue material;
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
contribution statements, scope, and established preference matches. System 1
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

The Codex adapter reuses one app-server process per provider across concurrent
candidates. Each issue starts an independent thread and receives one request containing
all seven questions, retaining each question's criteria and relevant material. Threads
share no issue conversation or main-agent chat. The adapter reuses the installed CLI
and authentication. Classification has no shell, search, filesystem, or other tools. It requests
`gpt-6-luna` with `reasoning.effort=none`, uses strict JSON Schema, checks protocol
confirmation of the requested settings, and validates the response again in Rust.
A mismatch fails the call rather than switching model or reasoning level.

## Configuration and failures

Configuration is optional. These defaults are independent of the legacy `[llm]`
configuration; enabling System 1 does not enable that old review path:

```toml
[system1]
concurrency = 4          # Positive number of in-flight candidates; no extra upper cap.
codex_binary = ""        # Discover an existing Codex CLI.
timeout_seconds = 45
candidate_budget = 24    # Hard cap per scout call; accepted range 0..100.
task_preferences = ""   # Explicit inclusion/exclusion preferences, if configured.
```

`ISSUE_FINDER_CODEX_BIN` can select an existing executable. Cloud setup should
supply the private installed CLI path explicitly. Local execution only discovers
and reuses a CLI/login; it does not install, upgrade, or rewrite global Codex
configuration. A zero candidate budget explicitly skips semantic screening.
Concurrency limits in-flight candidates rather than questions; requests never split
one issue into seven calls. The candidate budget caps issues screened, independently
of concurrency. Fresh facts exclude known unavailable work before model screening.
The semantic pool includes candidates near the ranking boundary; task-form labels
and legacy keyword interpretations do not permanently exclude otherwise eligible work.

Scout's `system1` output distinguishes completed answers, inability to answer,
provider failure, and budget-skipped candidates. Inspect each candidate's status,
`inputId`, `materials`, and `snapshotPath`, together with scout diagnostics.
Authentication failure, unavailable model, timeout, and invalid JSON or answer
values produce explicit partial results; they are not negative issue judgments.
A single candidate failure or cancellation is isolated from other issue threads.
The adapter retries at most once for an explicitly rejected overload RPC or structured
recoverable connection error, respecting retry timing plus random jitter within the
original timeout. Accepted-turn failures are conservatively reported without replay.
Lost responses trigger bounded status/interrupt cleanup, never an unconfirmed duplicate
submission. Process exit fails pending requests and is disclosed.
Diagnostics record concurrency, peak in-flight candidates, per-candidate queue and
execution time, model duration when available, and cache hits.
There is no fallback to the replaced semantic keyword rules. Logs go to stderr;
tool stdout remains a single JSON object.

Valid decisions can be reused for the same material, question definitions,
resolved preferences, and provider/model/reasoning configuration. Changes in any
of those inputs invalidate reuse. The decision cache has a six-hour freshness
limit; `refresh: true` bypasses reuse. This caches semantic interpretations only:
initial and final availability checks still fetch fresh GitHub facts on every scout
call. A semantic cache hit cannot establish that work remains available.

Full scout ranking replay is recorded separately from reusable model decisions.
`diagnostics.system1ReplayPath` identifies a snapshot under
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
System 1 request and reports semantic screening as not evaluated for the new context.
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

Optional probabilities must come from the provider's native service. Boolean
probability, mutually exclusive Choice distributions, and ordered Score
expectations have distinct meanings. The current Codex channel provides no
verified token probabilities, so probability information is absent. Generated
percentages, fabricated one-hot distributions, and repeated sampling are not
substitutes for provider probabilities.

See [installation and live verification](../skills/issue-finder-cli/references/install.md)
for the Cloud/local setup paths. Offline regressions must not depend on a live
model; `issue-finder system1-check` is a separate real-request acceptance check.
The [evaluation report](system1-evaluation.md) records historical v1 regression and
real-model checks, including the original 38/40 result. Those eight-question results
are preserved as historical evidence and do not establish v2 or concurrency acceptance.
Current offline fixtures are documented in the
[fixture guide](../tests/fixtures/system1_eval/README.md).
