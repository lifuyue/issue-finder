# CLI session smoke — 2026-09-08

This read-only smoke exercised the new current-agent tool contract against GitHub,
using the locally built CLI and an isolated `ISSUE_FINDER_HOME`. It did not clone a
target repository, edit an issue, record exposure/read feedback, or run dispatch.
Raw API responses, caches, and authentication details remain outside the repository.

## Checks and observations

- `status` returned `ready` with no config file, using the existing `gh` login.
  The response identified the credential source without returning the token.
- `scout` searched `astral-sh/ruff` with `query: label:bug`, `sort: updated`,
  `perPage: 3`, `maxPages: 1`, `apiBudget: 30`, and a Rust/linter/parser profile.
  It used 17 network requests, retrieved and assessed three issues, returned one
  visible ranked candidate and three explicitly unreviewed diagnostic candidates.
  It reported 205 GitHub matches and `nextPage: 2`; scanning one page was not
  described as exhausting the search.
- Optional recent-stargazer enrichment returned HTTP 404. Both scout and assess
  surfaced the failure and returned `partial`, preserving available evidence.
  This existing growth-data limitation was not hidden or converted into proof
  that a repository lacks activity.
- `assess` for [ruff #16410](https://github.com/astral-sh/ruff/issues/16410)
  returned the full issue body and all six comments, with
  `commentsTruncated: false` and no next comment page.

## Agent interpretation

The ranked candidate remained `needs_triage` and did not pass the default prepare
gate. Reading its complete discussion revealed both a concrete reproduction and
an unresolved scope question involving related diagnostics, including recent
investigation by another contributor. The current agent can now inspect this
evidence before deciding whether to continue. API ordering, a competition score,
or an entry in `diagnosticCandidates` alone does not establish that the issue is
available and suitable for implementation.

## Limits

This is a bounded integration smoke, not a six-profile recommendation quality
evaluation or a completed external contribution. No ranking weights or quality
thresholds changed in this implementation. Mock regressions cover the new search
controls, pagination, budgets, cache identity, unavailable issue context, full
comments, and current-agent lifecycle. Existing offline recommendation evaluation
remains the ranking regression baseline.

The smoke used source-built session contract version 1. It does not establish that
the latest published CLI package already contains this contract.
