# Frozen System 1 evaluation

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
cargo test --test system1_evaluation
```

The existing `recommendation_eval` datasets continue to cover control/dispatch compatibility;
this dedicated evaluator covers the replacement scout semantic path without rewriting
the old datasets or losing their comparison value.

This compares the deterministic consumer of recorded typed answers against the frozen
baseline, tests rejected-material leakage and the quality of the first five candidates,
and verifies JSON replay and neutral provider failure. It proves policy behavior given
answers; it does **not** prove a model can produce those answers.

Explicit live Luna acceptance (requires an already configured CLI and authentication):

```bash
ISSUE_FINDER_SYSTEM1_LIVE_EVAL=1 \
ISSUE_FINDER_SYSTEM1_EVAL_REPORT=/tmp/system1-live-eval.json \
cargo test --test system1_evaluation live_luna_classifies_frozen_github_material_and_reports_quality -- --ignored --nocapture
```

The default live subset is all five real snapshots. `ISSUE_FINDER_SYSTEM1_EVAL_IDS` can
select comma-separated sample IDs, including synthetic cases. The test sends actual
bounded evidence through the business question builder and Codex adapter. It reports
all current seven-question answer mismatches, persists responses/model metadata if a report path is supplied,
and requires exact agreement on task type, description quality and contribution state,
plus zero false hiding of good candidates. Additional-field disagreement remains visible
for rubric review; it is not silently counted as a pass. Do not change labels to fit model
outputs. Classifier failures or ambiguous rubrics should be investigated separately.

The initial live five-case run matched all 15 core fields and 38/40 fields overall.
The two additional-field disagreements and the unsupported frozen maintainer label
are preserved in [the evaluation report](../../../docs/system1-evaluation.md).
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
No additional live quality evaluation is part of this migration. A separate bounded
five-request concurrency acceptance checks transport behavior without scoring labels;
see the current section in [the evaluation report](../../../docs/system1-evaluation.md).
Schema 1 replay
files validate the original v1 question contract and preserve their captured ranking;
schema 2 captures explicitly identify v2 and recompute only under the current policy.
