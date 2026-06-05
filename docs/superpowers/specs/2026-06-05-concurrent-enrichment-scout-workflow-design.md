# Concurrent Enrichment And Scout Test Workflow Design

## Goal

Live `issue-finder.scout` quality checks currently spend most of their time enriching candidates serially. The recommendation quality policy made this more visible: a live run can spend roughly two minutes enriching 25 candidates, then return only a few displayable issues after strict filtering.

This change makes enrichment concurrent without changing discovery, value scoring, feed ranking, feedback state, prepare gates, or target-repo safety boundaries. It also documents the repeatable scout quality workflow used for live validation so future ranking changes are judged the same way.

## Current Bottleneck

`RecommendationEngine::rank_discovered_issues` performs:

1. discover GitHub issues;
2. rough local candidate rank;
3. truncate to `limit.clamp(25, ENRICHED_SCOUT_CANDIDATE_LIMIT)`;
4. enrich each rough candidate one by one;
5. value-score each enriched issue;
6. apply feed ranking and quality policy.

Step 4 is the expensive part. Each candidate may fetch repository metadata, issue details, comments, timeline references, stars, forks, and cache reads/writes. These requests are independent per issue, so bounded concurrency is safe.

## Selected Approach

Use bounded in-process concurrency with `futures::stream::buffer_unordered`.

Implementation details:

- Add `futures = "0.3"` to dependencies.
- Add `const ENRICHMENT_CONCURRENCY_LIMIT: usize = 4`.
- Keep the rough candidate order and enumerate before concurrent work.
- For each `(index, rough_issue)`, call the existing `rank_single_issue` future.
- Use `index < COMPETITION_TIMELINE_CANDIDATE_LIMIT` exactly as today, so timeline enrichment is still limited to the top rough candidates.
- Collect successful `RankedValueIssue` values into a vector.
- Continue to ignore per-candidate enrichment failures, matching current scout behavior.
- Apply feed ranking once after all candidate futures complete.

`buffer_unordered` is intentional: the final order is not determined by completion order. Feed ranking is applied after collection, so output remains deterministic for the same enriched data. This avoids needing `tokio::spawn`, `'static` lifetimes, cloned engines, or semaphore features.

## Non-Goals

This change does not:

- rewrite GitHub discovery queries;
- increase the candidate pool size;
- loosen quality filtering to fill the requested limit;
- change cache keys or cache TTLs;
- change prepare behavior;
- run scripts or modify target repositories.

Candidate recall remains a separate follow-up. If strict quality filtering returns only a few issues, the correct next fix is discovery/query recall, not weakening the feed quality policy.

## Failure And Rate-Limit Behavior

The concurrent implementation keeps the same failure semantics:

- a failed candidate enrichment is skipped;
- scout succeeds if at least some candidates can be assessed;
- no failed candidate writes feedback events;
- `recordExposure=false` still writes no shown events.

The concurrency limit stays deliberately low at 4 to avoid making GitHub API pressure worse. This is a fixed internal default for the first pass, not a new CLI or tool argument.

## Deterministic Test Workflow

Unit and integration tests must not depend on real GitHub, real tokens, real user state, or external network.

Required local verification:

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- tools list
```

Concurrency-specific tests:

- Add a mocked GitHub integration test with delayed enrichment endpoints.
- Make at least four candidate enrichments incur a small deterministic delay.
- Assert the scout runtime is materially below the serial upper bound.
- Assert final ordering remains feed-score driven, not completion-order driven.
- Assert a single delayed or failed enrichment does not fail the whole scout.

The timing test should use generous thresholds so it proves concurrency without becoming flaky on slow machines. It should compare against the mock's known artificial delay rather than a live GitHub duration.

## Live Scout Quality Workflow

Live validation is allowed for manual quality checks, but it is not part of automated tests.

Use:

```bash
GITHUB_TOKEN=$(gh auth token) \
cargo run --quiet -- tools call issue-finder.scout \
  --arguments '{"limit":10,"refresh":true,"includeFiltered":false,"recordExposure":false}' \
  --call-id live_scout_quality
```

Rules for interpreting live scout:

- Always use `recordExposure=false` for quality checks so validation does not pollute feedback state.
- Record `filteredCount`, candidate count, feed scores, quality penalties, visibility, risk tags, and issue refs.
- Fully read the returned issue bodies and comments before calling the result good or bad.
- If a bad issue is caused by an already claimed issue, submitted PR, low-depth docs task, profile mismatch, or broad audit/campaign, fix the quality policy.
- If only a few clean candidates remain after strict filtering, treat that as a recall/discovery problem.
- Keep final implementation verification deterministic with mocks even when live scout was used for product judgment.

## Acceptance Criteria

- Live scout enrichment is bounded-concurrent instead of serial.
- Existing ranking semantics remain unchanged after enrichment completes.
- Existing quality policy and feedback visibility rules still decide displayability.
- Mock tests prove concurrent speedup without real network.
- Mock tests prove completion order does not affect final feed order.
- The documented local and live test workflows match the commands used during implementation.
