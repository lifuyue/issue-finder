# Codex discovery and assessment tools

`issue-finder tools list` defaults to the session profile. Negotiate
`sessionContractVersion: 2`; only the following business tools are exposed.
`issue-finder tools --profile session list` is the explicit equivalent. MCP also
defaults to this profile. The separate legacy control catalog requires
`--profile control`; it is outside this skill's Codex contract.

Call with `issue-finder tools call NAME --arguments JSON`. Quote JSON safely or
use structured process arguments; never interpolate issue text into shell code.

| Tool | Purpose | Arguments |
| --- | --- | --- |
| `issue-finder.scout` | Discover, filter, and rank candidates | `repo`, `limit`, `search`, `profile`, `refresh`, `includeFiltered`, `recordExposure` |
| `issue-finder.assess` | Read one issue, discussion, competition, and repository evidence | `issue` or `url`, `commentsPage`, `commentsPerPage`, `profile`, `refresh`, `recordRead` |

The installed catalog is the source of truth for argument bounds and defaults.
`issue` uses `owner/repo#123`; `url` uses a GitHub issue URL. Supply one selector.
These calls neither prepare a workspace nor run target-project checks or publish
GitHub changes.

## Discovery parameters

`scout.search` accepts `query`, `sort`, `order`, `page`, `perPage`, `maxPages`, and
`apiBudget`. Omitting it selects the curated recommendation feed. An explicit
search is bounded GitHub retrieval followed by Issue Finder ranking. `sort` is
`updated`, `created`, `comments`, or `best_match`; `order` is `asc` or `desc`.
The explicit `repo` scope cannot be redirected by query qualifiers.

`profile.techStack`, `profile.keywords`, and optional `profile.taskPreferences`
override preferences for one call. `taskPreferences` is at most 4000 characters
and records explicit task preferences such as "avoid documentation polishing".
Carry scout's resolved `profile` into assessment and discussion pagination.
Omitting a field can inherit configured preferences; explicit empty arrays clear
that preference for the call. No profile bootstrap or unrelated chat scan is
required.

`includeFiltered` can expose filtered entries with reasons. Returned
`diagnostics.search.diagnosticCandidates`, when present, is a bounded pool of
retrieved but unrecommended issues. Assess plausible entries to fill evidence
gaps; do not present them as vetted recommendations.

Scout checks fresh initial GitHub availability, screens eligible candidates with
seven `scout-semantics-v2` questions, then freshly rechecks provisional results and
backfills from the bounded pool within the existing API budget. The default
provider is Alibaba `decision-model-preview`; Cloudflare `clef-flash` and Codex
app-server (`gpt-6-luna`, reasoning disabled) are explicitly selectable alternatives.
Each issue uses one seven-question request with each question's criteria and
material. The Codex fallback uses one process and independent issue threads.
Native providers preserve their service probabilities and confidence; these are
model outputs, not verified facts. Codex probability fields remain empty, and
provider errors do not cause an automatic model switch.
Default configuration concurrency is 4 and must be positive; there is no extra
upper cap. Single-candidate failures are isolated, with at most one retry for a
retryable server response within the original timeout. GitHub facts and semantic
interpretations remain distinct; deterministic policy consumes the answers.
`working`, `fix_claimed` and `conflicting` are soft evidence-check reminders, not
automatic contested categories. Task form does not exclude concrete documentation,
content, generated, event or rewarded changes; goal, clarity, scope and explicit
preferences determine their fit. Failed, unable-to-answer, and budget-skipped candidates remain
identifiable. There is no fallback to replaced semantic keyword hiding. See the
[Decision model guide](../../../docs/decision.md) for configuration and the provider contract.

For the configured Cloudflare provider, select an injected token by environment
key name for one process; never put the token value in the selector:

```bash
ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV=cf-fallback-1 issue-finder decision-check
ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV=cf-fallback-1 issue-finder scout --repo owner/repo
ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV=cf-fallback-1 issue-finder tools call issue-finder.scout --arguments '{"repo":"owner/repo"}'
```

The token must already be injected under `cf-fallback-1`. Omitting the selector
uses `CLOUDFLARE_API_TOKEN`; a present selector must be nonempty and contain only
ASCII letters, digits, underscores, or hyphens. A missing/empty selected token or
invalid name returns an error without automatically trying another
key. There is no selector CLI flag or TOML setting. If the selected token belongs
to another account, also set `CLOUDFLARE_ACCOUNT_ID` to the matching account ID.
See the [provider guide](../../../docs/decision-providers.md) for startup setup.

## Outputs and freshness

Inspect the full JSON envelope: `success`, `status`, warnings, and error details.
Tool data is in `structured_content`:

- Scout: `candidates`, `diagnostics.search`, `apiBudget`, and resolved `profile`.
  Each candidate's `decision` exposes semantic status/answers, input hash, material
  scope, question version and saved snapshot path; `availability` contains fresh
  issue/repository/PR facts and coverage. Diagnostics disclose failures, skips,
  concurrency, peak in-flight work and per-candidate timing/cache hits.
- Assess: `issue` with body and one discussion page, `assessment`, `competition`,
  `repository`, `activity`, `assessmentFetchedAt`, fresh final-depth `availability`,
  and warnings when available. Assess makes no model request.
  `issueWarnings` identifies a locked discussion or existing assignee.
- Discussion pagination: `issue.comments`, `nextCommentsPage`,
  `commentsTruncated`, and `totalComments`.

`no_candidates` means no recommendation from the performed search, not that
GitHub has no matching issues. `issue_unavailable` identifies a closed issue or a PR reference and includes its
reason and context. Locked or assigned open issues remain assessable; inspect
`issueWarnings` before proceeding. `partial` discloses missing evidence or failed
assessment work. Neither
partial evidence nor a competition score establishes the absence of competing
PRs. `availability.pull_requests` preserves repository/number, verified API state,
merge facts, base branch, relation and sources. Closed does not mean merged.
Explicit closing directives targeting the issue establish resolution relationships;
mentions and search matches remain leads. Merged resolution evidence for this
repository's default branch requires inspecting current code, not declaring a fix.
Incomplete coverage and unresolved relationships remain unknown. Assessment
reasons, including popularity, are advisory information rather
than repair authorization gates.

Use `refresh: true` to bypass cached discovery/assessment evidence when stale
information affects a decision. Assess retrieves the selected issue/discussion
page on each call; refresh additionally updates cached assessment evidence.
Assess marks semantic judgment as not evaluated for its new context; an old scout
answer is historical screening, not freshly verified evidence. Scout cache reuse
is tied to materials, questions, resolved preferences, and provider/model/reasoning
configuration, with a six-hour freshness limit. Semantic reuse never replaces fresh
initial/final GitHub availability checks. Old schema 1 eight-question replays validate
v1 and preserve their captured outcome without v2 recomputation; current schema 2
captures explicitly identify v2. Historical reports do not establish v2 acceptance.
Read further pages only when needed. Honor rate limits rather than retrying
through parallel clients.

Shown/read events are recorded automatically unless `recordExposure: false` or
`recordRead: false` opts out. Historical dismissed/done/prepared states do not
hide or influence Codex recommendations. The CLI retains no Codex workspace task,
validation result, or PR completion record. Codex owns those operations through
its normal tools and chat context.

Configuration and authentication failures are returned directly by business
calls. `GH_TOKEN` is the sole GitHub credential environment key. Session tools
retain optional `[github].token` and captured stored `gh` login as fallbacks; tokens are not printed or persisted by the
fallback. GitHub reads and contribution publication have distinct permissions.
