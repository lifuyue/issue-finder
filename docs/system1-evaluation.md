# System 1 evaluation — 2026-10-01

> The quality results below describe the historical eight-question
> `scout-semantics-v1` run. They are not quality or concurrency results for the current
> seven-question v2 policy. Original fixtures and reports remain unchanged.

## Current v2 concurrency acceptance

The [five-request transport acceptance](evidence/system1-concurrency-2026-10-01.json)
used the same five frozen public issue materials with seven v2 questions per request.
Codex CLI 0.159.3 ran one app-server process and five independent temporary threads.
Protocol acknowledgements show four active turns before the first completion; the
fifth started after a slot was released. All five responses passed schema and semantic
contract validation. Total wall time was 13.139 seconds; the fifth request waited
7.179 seconds for its slot. Only protocol identities and timing were traced.

This is a bounded concurrency acceptance, not a new quality evaluation or a measured
speedup comparison. The historical serial timing below used a different question
version and cannot establish a speedup factor. Routine tests skip both live checks.

## Historical v1 quality evidence

The frozen regression set contains **5 unmodified public GitHub title/body/comment
snapshots and 14 explicitly synthetic edge examples**. Labels were written before the
live model run and were not changed to match its outputs. Repository facts and a
matched Rust/TypeScript CLI profile are held constant to isolate semantic policy;
these results are not the issues' actual live repository ranks.

| Evaluation | Result | What it establishes |
| --- | --- | --- |
| Frozen old pipeline, revision `d970c836aecf3be808206ae21e48029ea21ee418` | 11 good intended-visible candidates hidden | Actual old `assess_issue`/quality policy, executed from an isolated archive |
| New policy with hand-labelled semantic answer snapshots, 19 examples | 0 good candidates falsely hidden; first 5 are appropriate; reject remains hidden | Deterministic policy and downstream feed consumption; **not model classification accuracy** |
| Actual Luna calls, 5 real snapshots | 15/15 core answers correct; 0 good candidates hidden | Live task type, description quality and contribution-state recognition |
| Actual Luna calls, all 8 questions | 38/40 match frozen labels | Ancillary disagreements remain recorded for rubric review |

Offline tests also verify replay, neutral failure, incomplete evidence and that old
keyword-derived comment counters cannot re-hide candidates or add another penalty.
Existing recommendation fixtures remain for control/dispatch compatibility.

The live run used Codex CLI **0.159.3**, protocol-confirmed **`gpt-6-luna`** and
**`reasoning.effort=none`**. Native usage reported zero reasoning output tokens on all
five turns. Per-turn latency was 6.4–9.7 seconds; total test time was 37.12 seconds.
Probabilities were absent. Routine tests do not call GitHub or a model service.

A separate [live scout integration check](evidence/system1-scout-2026-10-01.json)
retrieved rust-clippy #17824 from GitHub and completed all eight questions through
the production session tool. Model time was 7.16 seconds (13.35 seconds end to end),
with zero reasoning tokens, single JSON stdout and a saved full ranking replay.
The overall result correctly remained `partial`: GitHub throttled the optional
recent-stargazer sample. This smoke check is not another labelled quality sample.
The tool catalog still contained exactly `scout` and `assess`, contract version 2.

The real source issues are [context-drift #4](https://github.com/7shep/context-drift/issues/4),
[rust-clippy #16858](https://github.com/rust-lang/rust-clippy/issues/16858),
[clap #5919](https://github.com/clap-rs/clap/issues/5919),
[clap #3910](https://github.com/clap-rs/clap/issues/3910), and
[rust-clippy #17398](https://github.com/rust-lang/rust-clippy/issues/17398).
They cover clear documentation, interest and participation inquiries, quoted speech,
explicit active work, and a claim followed by the same author's withdrawal.

Both ancillary disagreements occurred in rust-clippy #16858:

- `maintainer_signal`: label `encouraged`, model `not_observed`. The retained GitHub
  association is `CONTRIBUTOR`, so the label's assumption that the encouraging author
  is a maintainer is unsupported. The model follows the association-based criterion.
- `scope`: label `bounded`, model `design_needed`. The requested lint is specific,
  but deciding whether a broad new restriction lint should be accepted can reasonably
  require design discussion. The frozen label does not capture that ambiguity.

This small acceptance set supports the tested semantic distinctions. The 19-example
policy result uses gold answers, and the five live calls do not establish broad model
accuracy or measured reductions in main-agent reading time. Synthetic negation,
multiple-actor and incomplete-timeline cases are offline policy regressions here,
not additional live model classifications.

The [raw acceptance report](evidence/system1-live-2026-10-01.json) retains each response,
question/input IDs, typed answers, disagreements, native usage and confirmed metadata.
It contains no raw body duplication or credentials. It includes fixture/baseline
hashes and a post-run source hash manifest; this is distinct from claiming every
post-run logging edit was executed by the live run.

Frozen dataset SHA-256:
`5d2d604482890adf5187743da991537bc061b0d48f8ea13bec8c472fc16b75cb`.
See [fixture provenance and rubric](../tests/fixtures/system1_eval/README.md).

Offline reproduction:

```bash
cargo test --test system1_evaluation
```

The exact acceptance command was:

```bash
ISSUE_FINDER_CODEX_BIN=/home/agent/.local/bin/codex \
ISSUE_FINDER_SYSTEM1_LIVE_EVAL=1 \
ISSUE_FINDER_SYSTEM1_EVAL_REPORT=/workspace/system1-live-eval.json \
cargo test --test system1_evaluation live_luna_classifies_frozen_github_material_and_reports_quality -- --ignored --nocapture
```

A configured local CLI can omit `ISSUE_FINDER_CODEX_BIN`; the adapter then discovers
it without installation or global configuration changes. Report output may use a
local temporary path. The committed report above preserves this acceptance evidence.
