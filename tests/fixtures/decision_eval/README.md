# Frozen decision model evaluation

`samples.json` was hand-labelled before any Luna acceptance run. It contains five
raw public GitHub snapshots and fourteen explicitly synthetic regression examples.
Titles, bodies and comments in `real_github_snapshot` records are unmodified API
material, with public URLs, comment IDs, actors, author associations and timestamps.
Only the bounded fields needed for these questions are retained. Synthetic examples
have `example.invalid` provenance and must never be reported as observed GitHub issues.

The real snapshots are:

- context-drift #4: an exact README flag/range/default correction.
- rust-clippy #16858: participation inquiry, quoted by another contributor who invites work.
- clap #5919: an inquiry about whether anyone is working, followed by interest and encouragement.
- clap #3910: a contributor explicitly describes current implementation and regression coverage.
- rust-clippy #17398: the sole claimant later unclaims; another author's use of “claim” refers to the lint assertion.

The synthetic set covers denied and quoted speech, separate contributors, withdrawals,
unordered contradictory statements, an unverified local fix claim, confirmed open and
closed PR facts, missing/sampled comments, empty templates, long vague text, short clear
documentation and truncated descriptions. Each sample has `expected.quality`,
`expected.behavior`, human reasons and typed semantic answers.

Repository facts are deliberately held constant (10,000 stars, 1,500 forks, active
Rust/TypeScript CLI repository, matched profile). This isolates semantic policy from
source-repository influence, freshness and actual historical issue state. It does
**not** claim these issues would have the same live rank in their original repositories.
An actual open PR remains a factual hiding constraint.

`legacy_baseline.json` records the real old algorithm's results, captured by executing
`assess_issue` and `assess_quality_policy` from an isolated `git archive` of revision
`d970c836aecf3be808206ae21e48029ea21ee418`. It uses the same controlled facts and raw
comment text as the new evaluation, including old keyword-derived counters. Baseline
results are frozen rather than recomputed through code being replaced. The old pipeline
falsely hid 11 hand-labelled good intended-visible candidates. These include four of the
five real snapshots (one active contribution is intended to remain visible at lower priority).

Offline regression:

```bash
cargo test --test decision_evaluation
```

The existing `recommendation_eval` datasets continue to cover control/dispatch compatibility;
this dedicated evaluator covers the replacement scout semantic path without rewriting
the old datasets or losing their comparison value.

This compares the deterministic consumer of recorded typed answers against the frozen
baseline, tests rejected-material leakage and the quality of the first five candidates,
and verifies JSON replay and neutral provider failure. It proves policy behavior given
answers; it does **not** prove a model can produce those answers.

Manual native transport smoke/performance now lives outside the offline test suite:

```bash
cargo run --example decision_benchmark -- smoke --provider aliyun-decision --report /tmp/decision-smoke.json --live
cargo run --example decision_benchmark -- concurrency --provider aliyun-decision --report /tmp/decision-concurrency.json --live
cargo run --example decision_benchmark -- workload --report /tmp/decision-workload.json
```

Smoke uses at most the five frozen real snapshots; `--ids` selects real snapshot IDs.
Concurrency uses 24 repeated logical calls per run in order `[4, 8, 8, 4]`, capped at
96 logical calls. Both require explicit `--live`; workload export is offline. Native
provider endpoint/credential environment variables are the same as the production CLI.
The historical live Luna quality experiment and five-call transport experiment were
retired. Their recorded evidence and original labels remain unchanged.

The initial live five-case run matched all 15 core fields and 38/40 fields overall.
The two additional-field disagreements and the unsupported frozen maintainer label
are preserved in [the evaluation report](../../../docs/decision-evaluation.md).
The original labels remain unchanged.

Initial frozen `samples.json` SHA-256:
`5d2d604482890adf5187743da991537bc061b0d48f8ea13bec8c472fc16b75cb`.


## Semantic v2 migration

The current offline evaluator uses `samples_v2.json`, with seven semantic questions.
It explicitly records the v1 fixture reference, removes `task_shape` from current
answers, and adds bounded content, exercise, dashboard generator, generated template,
event and rewarded changes. Task form does not filter these concrete changes.
The empty template remains uncertain and visible for triage under v2. Discussion
expressions such as working, fix claimed and conflicting request evidence checks;
GitHub facts determine availability and competition.

`samples.json`, the original eight-question labels, legacy baseline and initial
38/40 live report remain historical artifacts. They are not relabelled as v2 results.
No additional live quality evaluation is part of this migration. The manual native smoke example
checks transport behavior without scoring model quality;
see the current section in [the evaluation report](../../../docs/decision-evaluation.md).
Schema 1 replay
files validate the original v1 question contract and preserve their captured ranking;
schema 2 captures explicitly identify v2 and recompute only under the current policy.
