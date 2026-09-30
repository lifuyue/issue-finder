# Current-session CLI tool mapping

Negotiate the installed catalog with `issue-finder tools --profile session list`.
Call tools with `issue-finder tools --profile session call NAME --arguments JSON`.
Use proper shell quoting or the host's structured process arguments for JSON;
never interpolate issue content into executable shell text.

| Agent step | Tool | Purpose |
| --- | --- | --- |
| Prerequisites | `issue-finder.status` | Configuration and authentication diagnostics without token values |
| Discover | `issue-finder.scout` | Bounded GitHub search, candidate enrichment, and contribution ranking |
| Review | `issue-finder.assess` | One issue and paginated discussion without preparing a workspace |
| Start work | `issue-finder.prepare` | Fresh assessment and workspace/task preparation |
| Continue | `issue-finder.task_status` | Reopen a task by absolute workspace and check identity |
| Validate and record | `issue-finder.finish` | Explicit check execution and task/result validation |
| Feedback | `issue-finder.feedback` | Record read, dismiss, or restore for an issue |

`scout.search` separates retrieval from ranking: `query`, `sort`, `order`, `page`,
`perPage`, `maxPages`, and `apiBudget` bound the GitHub work. `scout.profile` and
`assess.profile` take `techStack` and `keywords` without changing saved preferences.
Carry the resolved scout response's `profile` into every assess and prepare call;
omitting it reverts that invocation to configured defaults.
An empty profile override is not necessarily neutral. Inspect and disclose the
resolved defaults rather than attributing them to the user.
Search ordering is not the final ranking. Preserve explicit repository scope and
read warnings, incomplete evidence, and budget details before expanding search.

`assess` accepts `commentsPage` and `commentsPerPage`; request further pages only
when their discussion is relevant to selection or implementation. `prepare`
rechecks fresh issue state so an earlier cached recommendation cannot authorize
work on a changed or competing issue.
Neither its score nor a successful gate replaces reading relevant comments and
checking for unlinked competing PRs and current-code feasibility.

`prepare.checkout`, `prepare.workspaceRoot`, `task_status.workspace`, and
`finish.workspace` are absolute filesystem paths. Use returned paths rather than
guessing workspace naming. The agent owns code edits; the CLI owns generated
task/result data. Never edit task metadata to force a successful finish.

`status` checks read/authentication readiness, not the full fork/push/PR chain.
`finish` records local changes and checks; it does not create or track a PR,
certify upstream CI, or establish that a contribution has merged. The current
agent owns authorized GitHub publication and reports those outcomes separately.
See [GitHub delivery](github-delivery.md).

Tool data is in `structured_content`. Scout returns `candidates`,
`diagnostics.search`, and `apiBudget`; assess returns `issue` with the complete
body and a discussion page (`comments`, `nextCommentsPage`, `commentsTruncated`,
`totalComments`). Prepare returns `taskFile`, `task.taskId`, `task.workspace`,
and `task.evidence.issueContext`. Its `.issue-finder-cli-task.json` is distinct
from the independent skill's task file. Finish returns `changes.changedFiles`,
`checks[].outputTail`, and `resultFile`. The CLI retains the task for continuation
and keeps the canonical task/result under its session state directory.

When present, `diagnostics.search.diagnosticCandidates` contains a bounded pool
of retrieved issues excluded from recommendations. Read their filtering reasons
and assess plausible entries to fill evidence gaps; these are diagnostic leads,
not recommended candidates.

Business stops and tool failures are distinct. Inspect the full JSON envelope,
including `status` and any error code/details. A gate block does not imply a
prepared task; an empty or incomplete search does not prove no matching issues
exist. Correct invalid arguments from the negotiated schema, authenticate once
if necessary, honor rate-limit information, and retry only when a changed input
or resolved failure can produce useful progress.

There is no `run_id`, dispatch approval, worker package, or nested agent in this
profile. Do not switch to `submit_result`, `inbox done`, or a dispatch command to
work around a failed finish.
