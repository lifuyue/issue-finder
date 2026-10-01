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

`profile.techStack` and `profile.keywords` override preferences for one call.
Carry scout's resolved `profile` into assessment and discussion pagination.
Omitting a field can inherit configured preferences; explicit empty arrays clear
that preference for the call. No profile bootstrap or unrelated chat scan is
required.

`includeFiltered` can expose filtered entries with reasons. Returned
`diagnostics.search.diagnosticCandidates`, when present, is a bounded pool of
retrieved but unrecommended issues. Assess plausible entries to fill evidence
gaps; do not present them as vetted recommendations.

## Outputs and freshness

Inspect the full JSON envelope: `success`, `status`, warnings, and error details.
Tool data is in `structured_content`:

- Scout: `candidates`, `diagnostics.search`, `apiBudget`, and resolved `profile`.
- Assess: `issue` with body and one discussion page, `assessment`, `competition`,
  `repository`, `activity`, `assessmentFetchedAt`, and warnings when available.
  `issueWarnings` identifies a locked discussion or existing assignee.
- Discussion pagination: `issue.comments`, `nextCommentsPage`,
  `commentsTruncated`, and `totalComments`.

`no_candidates` means no recommendation from the performed search, not that
GitHub has no matching issues. `issue_unavailable` identifies a closed issue or a PR reference and includes its
reason and context. Locked or assigned open issues remain assessable; inspect
`issueWarnings` before proceeding. `partial` discloses missing evidence or failed
assessment work. Neither
partial evidence nor a competition score establishes the absence of competing
PRs. Assessment reasons, including popularity, are advisory information rather
than repair authorization gates.

Use `refresh: true` to bypass cached discovery/assessment evidence when stale
information affects a decision. Assess retrieves the selected issue/discussion
page on each call; refresh additionally updates cached assessment evidence.
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
