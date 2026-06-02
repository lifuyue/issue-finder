# Value Issue Test Coverage Design

## Summary

Patchbay's value ranking now depends on deterministic local interpretation of GitHub metadata. The current tests cover individual pieces, but they do not yet provide a clear scenario matrix for judging whether an issue is high value, low value, risky, actionable, established, or growth-oriented.

This design adds focused test coverage for the high-value issue algorithm, with two layers:

- fast scenario tests built from local `EnrichedIssue` fixtures
- a small number of mocked GitHub integration tests that exercise enrichment through `workflow::scout`

The goal is confidence in the scoring semantics without making the test suite slow or brittle.

## Goals

- Increase coverage for local value signals and score aggregation.
- Cover the main opportunity types:
  - `established_project`
  - `growth_project`
  - `balanced`
  - `niche_but_actionable`
  - `low_signal`
- Cover recommendation thresholds:
  - `strong_candidate`
  - `candidate`
  - `weak_candidate`
  - `avoid`
- Cover execution gate behavior around the `40` auto-prepare threshold.
- Cover risk penalties from stale activity and high-noise issues.
- Cover missing evidence behavior when star, fork, or comment samples are unavailable.
- Add end-to-end mock GitHub tests for ranking high-value issues after enrichment.
- Keep the tests deterministic and independent from real GitHub or LLM services.

## Non-Goals

- Do not change the scoring model in this work.
- Do not add property-based testing yet.
- Do not add snapshot or golden JSON files.
- Do not call real GitHub APIs.
- Do not call real LLM APIs.
- Do not expand CI beyond the existing macOS Rust checks.
- Do not assert exact total scores where a range or classification is more robust.

## Current Gaps

Existing tests cover:

- star and fork velocity helpers
- enrichment cache behavior
- enrichment tail page sampling
- partial enrichment failure
- basic signal presence
- basic score aggregation
- balanced opportunity classification
- daily skip for low execution gate

Missing coverage:

- full scenario matrix for opportunity type classification
- full scenario matrix for recommendation thresholds
- high-impact but low-actionability issue should not become an auto-prepare candidate
- high-growth but small repository should still score as a growth opportunity
- established and growth signals together should become balanced
- niche but actionable issues should be accepted when impact is modest but gate is high
- stale or noisy issues should surface risks and reduce value
- missing samples should produce missing evidence without crashing scoring
- profile fit should materially affect score and explanation
- mocked GitHub enrichment should change ranking based on repository and activity facts, not only search result order

## Test Architecture

### Fixture Builder

Add reusable test fixture helpers for value assessment scenarios.

Preferred location:

```text
tests/value_assessment_scenarios.rs
```

The fixture should build `EnrichedIssue` without network calls.

Builder capabilities:

- base issue fields:
  - title
  - body
  - labels
  - created and updated timestamps
- repository fields:
  - stars
  - forks
  - subscribers
  - open issues
  - pushed timestamp
  - topics
  - language
- activity fields:
  - recent issue activity
  - recent repository activity
  - maintainer recent response
- growth samples:
  - recent stargazers
  - newest forks
  - missing star sample
  - missing fork sample
- comment facts:
  - maintainer comment
  - high comment count
  - missing comment excerpts
- profile config:
  - matching tech stack and keywords
  - non-matching profile

The builder should keep dates relative to the current run where possible so freshness-based tests remain stable.

### Scenario Matrix Tests

Add scenario tests that assert classifications and important signal presence. Avoid overfitting to exact scores unless the threshold itself is the behavior under test.

Recommended scenarios:

1. **Established Project**
   - repo has high stars or forks
   - actionable issue body
   - recent repo or issue activity
   - expected:
     - `opportunity_type = established_project`
     - recommendation is at least `candidate`
     - includes `established_impact`

2. **Growth Project**
   - modest total stars
   - strong recent star or fork sample
   - actionable issue body
   - expected:
     - `opportunity_type = growth_project`
     - includes `growth_momentum`
     - `growth_confidence` is not `low` when timestamps exist

3. **Balanced Project**
   - established repository
   - strong recent growth sample
   - expected:
     - `opportunity_type = balanced`
     - includes both `established_impact` and `growth_momentum`

4. **Niche But Actionable**
   - modest impact
   - clear good-first/actionable issue
   - strong profile match
   - expected:
     - `opportunity_type = niche_but_actionable`
     - `execution_gate_score >= 60`
     - recommendation is at least `candidate`

5. **Low Signal**
   - low impact
   - no growth sample
   - vague issue body
   - weak profile match
   - expected:
     - `opportunity_type = low_signal`
     - recommendation is `weak_candidate` or `avoid`
     - missing evidence includes star and fork sample notes

6. **Avoid Low Gate**
   - high impact repository
   - issue body is vague and not actionable
   - no good-first label
   - expected:
     - `execution_gate_score < 40`
     - recommendation is `avoid` or `weak_candidate`
     - daily should not auto-prepare it

7. **Risk Penalty**
   - stale issue and stale repository
   - high comment count or many open issues
   - expected:
     - includes `staleness_risk`
     - includes `noise_risk`
     - risks are present in `ValueAssessment`

8. **Profile Fit**
   - same issue assessed with matching and non-matching profile
   - expected:
     - matching profile includes `issue_fit`
     - matching profile has higher `value_score`
     - explanation mentions matched profile terms

### Mock GitHub Integration Tests

Add a small integration test file:

```text
tests/github_value_mock.rs
```

The test server should return:

- GitHub search responses with multiple issues
- per-repository metadata
- issue details
- comments
- stargazer samples
- fork samples

Recommended integration scenarios:

1. **Scout Reorders By Value**
   - search returns a low-value issue first and a high-growth or high-impact issue second
   - enrichment makes the second issue score higher
   - expected:
     - `workflow::scout(...)[0]` is the high-value issue
     - top candidate has expected opportunity type and recommendation

2. **Scout Keeps Growth Candidate Competitive**
   - modest repository with strong recent star/fork velocity
   - established repository with weaker actionability
   - expected:
     - growth candidate is ranked above weakly actionable established candidate

3. **Daily Gate Still Applies**
   - high value but low gate candidate appears before actionable candidate
   - expected:
     - `daily_from_ranked` or a mock-backed daily path skips low gate candidate
     - prepared report contains the actionable candidate

The mock server should be deliberately small. If a scenario can be tested with local fixtures, keep it in the scenario test file rather than growing the HTTP mock.

## Assertions

Prefer assertions like:

- exact `OpportunityType`
- exact `Recommendation` only when testing thresholds
- `execution_gate_score >= 40` or `< 40`
- `value_score` comparison between two scenarios
- signal kind is present
- risk text is present
- missing evidence contains the expected note
- ranking order between two candidates

Avoid assertions like:

- exact total score for every scenario
- exact full explanation order for every scenario
- full JSON snapshots

This keeps tests stable when the scoring weights are adjusted intentionally.

## Determinism

Tests should not depend on real wall-clock dates except through helpers that generate current timestamps. Use current-time helpers to create:

- recent timestamps
- stale timestamps
- 7-day star samples
- 30-day fork samples

Network integration tests must use local TCP mock servers only.

## Error Handling Coverage

The enhanced tests should confirm:

- partial enrichment failures still produce an assessment
- missing star/fork/comment evidence is represented in `missing_evidence`
- low gate candidates are not auto-prepared by daily
- high-risk candidates expose risks rather than silently lowering scores

## Acceptance Criteria

- New tests cover all five opportunity types.
- New tests cover all four recommendation classes or the threshold behavior that produces them.
- New tests cover growth momentum from star/fork samples.
- New tests cover established impact from stars/forks/subscribers.
- New tests cover risk factors and missing evidence.
- At least one mock GitHub test proves enrichment can reorder scout results by value.
- Tests do not require real GitHub, real LLM services, or user-specific local state.
- The full suite passes:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`

## Implementation Notes

- Start with fixture-based tests. They will reveal algorithm assumptions quickly and keep failures readable.
- Add only the smallest mock GitHub server needed to prove the enrichment-to-ranking path.
- Reuse existing local server patterns from `tests/github_mock.rs` and `tests/enrichment_cache.rs`.
- If duplicated fixture code grows too much, extract helper functions within the test file first. Avoid adding production abstractions only for tests.
