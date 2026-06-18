# Hybrid Contribution Memory Design

Date: 2026-06-18

Status: Draft for review

## Summary

Issue Finder should add a full hybrid contribution memory system that combines two complementary ideas:

- Dreaming-style asynchronous consolidation: background synthesis, reviewable summaries, stale-memory correction, top-of-mind controls, and user-owned memory management.
- Akasha-style truth-preserving recall: raw events as the source of truth, role/source/trust preservation, direct recall plus graph ripple, per-moment activation scoring, anti-hub controls, and conservative write-back.

This memory system is not a general chat memory. It is a vertical contribution memory for open-source work. It should improve recommendation quality, dispatch success, GitHub interaction quality, and profile learning while preserving Issue Finder's local-first and offline-capable boundaries.

Raw events are facts. Indexes, embeddings, graph edges, dreams, and hints are derived. LLMs may propose dreams and candidate hints, but they must not rewrite raw history or directly change stable profile, ranking, dispatch, or GitHub behavior. Only approved memory hints may affect decisions.

## Goals

- Store contribution-related raw memory events without rewriting them.
- Preserve role, source, trust level, evidence refs, and deletion/tombstone semantics.
- Build rebuildable indexes over raw events: FTS, entity, rare-token, and optional embedding.
- Build a contribution graph connecting events, entities, agents, repos, maintainers, labels, issue types, validation paths, successes, and failures.
- Implement Akasha-style activation: direct recall, ripple recall, score settlement, resource suppression, anti-hub penalties, and write-back guards.
- Implement Dreaming-style consolidation: reviewable profile/repo/agent/discovery/stale/conflict candidates.
- Add authority controls: approve, reject, pin, deprioritize, suppress, tombstone, restore prior version, memory off, and no-write mode.
- Integrate approved memory hints into recommendation ranking, dispatch planning, IssueTaskPackage memory context, GitHub draft wording, and profile suggestions.
- Add offline memory evals that test recall, stale correction, deletion, over-recall, graph value, anti-hub behavior, and write-back safety.

## Non-Goals

- Do not build a general personal-life memory system.
- Do not ingest arbitrary full chat transcripts by default.
- Do not let LLM output directly modify `config.toml`, recommendation ranking, dispatch policy, or GitHub comments.
- Do not replace deterministic recommendation scoring, prepare gate rules, or existing feedback events.
- Do not require memory for `scout`, `assess`, `prepare`, `daily`, handoff generation, or recommendation eval.
- Do not require external vector databases, graph databases, real Codex, real GitHub write access, or real LLM services in automated tests.
- Do not expose private raw event payloads in GitHub drafts or public artifacts.

## Design References

- Akasha: https://kachofugetsu09.github.io/memory/akasha.html
- OpenAI Dreaming V3: https://openai.com/index/chatgpt-memory-dreaming/

### Dreaming Constraint

The memory system should follow these Dreaming-style principles:

- Consolidation happens asynchronously or explicitly, not inline on every recommendation.
- Summaries and hints are reviewable.
- User controls exist for delete, correction, prioritization, deprioritization, and memory-off behavior.
- Memory must handle staleness over time instead of treating old context as permanently current.
- Memory can help with factual recall, preference following, and time-sensitive currentness, but it must stay inspectable.

### Akasha Constraint

The memory system should follow these Akasha-style principles:

- Raw event/message is the source of truth.
- Role, source, trust, and evidence must be preserved.
- Retrieval is not memory. Memory asks which past context should surface now.
- Direct recall anchors graph ripple so associative recall can be broad without becoming noisy.
- Importance is not one static field. Activation is settled at query time from direct match, ripple, salience, strength, recency, resource, hub, trust, hop, and stale factors.
- Write-back is conservative. Pure ripple noise must not reinforce the graph.
- LLMs must not rewrite the past, independently decide importance, or solidify guesses as stable memory.

## Product Boundary

Hybrid contribution memory serves four product chains:

```text
recommendation quality
dispatch success
GitHub interaction quality
profile learning
```

It does not replace execution-agent memory. Codex, Claude Code, OpenHands, and other execution agents keep their native session memory. Issue Finder stores contribution memory about what was selected, attempted, approved, rejected, fixed, failed, and learned.

## Architecture

```text
Raw Contribution Event Ledger
  -> Memory Nodes
  -> Memory State
  -> Indexes
  -> Contribution Graph
  -> Activation Engine
  -> Write-back Guard
  -> Dreaming Consolidation
  -> Authority Gate
  -> Approved Memory Hints
  -> ranking + dispatch + GitHub draft + profile candidates
```

Layer roles:

```text
Raw Layer
  Stores facts. Not rewritten by LLM.

Node Layer
  Represents raw events, entities, episodes, dreams, and hints as activatable units.

State Layer
  Stores salience, strength, resource, recall counts, reinforcement counts, fan-in/out, and timestamps.

Index Layer
  Rebuildable FTS, entity, rare-token, and optional embedding indexes.

Graph Layer
  Typed relationships between memory nodes.

Activation Layer
  Produces per-query traces explaining why memories surfaced.

Write-back Layer
  Updates strength/resource/edges only when strict guards pass.

Dream Layer
  Produces candidate summaries and hints from activation traces and raw evidence.

Authority Layer
  Lets users or deterministic policy approve, reject, pin, deprioritize, suppress, tombstone, or restore.

Hint Layer
  Approved decision inputs consumed by ranking, dispatch, GitHub draft, and profile suggestions.
```

## Data Model

The memory schema should live in the same local SQLite database as dispatch state if practical, but under a distinct memory module boundary. It must remain usable without dispatch adapters.

### `memory_sources`

Tracks where a raw memory came from.

```text
id
source_type        # recommendation_event, dispatch_event, github_interaction, profile_bootstrap, manual
source_ref         # stable pointer to original row/artifact/file
trust_level        # user_explicit, system_observed, external_github, agent_observed, llm_inferred
created_at
```

### `memory_raw_events`

Stores raw contribution facts.

```text
id
source_id
event_type         # approve, reject, dismiss, dispatch_success, dispatch_failure, maintainer_reply, validation_pass, validation_fail
role               # user, system, agent, github, llm
trust_level
subject_type       # issue, repo, agent, maintainer, label, validation, profile
subject_ref
payload_json
confidence
occurred_at
created_at
tombstoned_at
```

Rules:

- `payload_json` is raw evidence or a minimal faithful projection of raw evidence.
- LLM-generated events must have `role = llm` and `trust_level = llm_inferred`.
- User feedback must have stronger trust than agent or LLM observations.
- Tombstoned raw events must not participate in index rebuilds, graph activation, dreams, or hints.

### `memory_nodes`

Represents any activatable memory unit.

```text
id
node_type          # raw_event, entity, episode, claim_candidate, dream, hint
raw_event_id nullable
entity_type nullable
entity_value nullable
normalized_value nullable
text_ref nullable
metadata_json
created_at
tombstoned_at
```

Node examples:

```text
raw_event: "user rejected owner/repo#123 as too broad"
entity: repo:owner/repo
entity: issue_type:rust_cli_panic
entity: failure_reason:unclear_validation
episode: "June 2026 Codex dispatch attempts on Rust CLI bugs"
dream: profile adjustment candidate
hint: approved ranking preference
```

### `memory_node_state`

Stores long-term and short-term state for each node.

```text
node_id
salience
strength
resource
recall_count
reinforce_count
fan_in
fan_out
last_recalled_at
last_reinforced_at
updated_at
```

Meanings:

```text
salience
  How notable the event was when born.

strength
  How durable this memory has become through successful use.

resource
  Short-term availability. Recently used memories spend resource and temporarily step back.

fan_in / fan_out
  Used to detect generic hubs that should not dominate explanations.
```

### `memory_indexes`

Stores rebuildable index references.

```text
node_id
index_type         # fts, embedding, rare_token, entity
index_ref_or_payload
created_at
```

Embedding support should be optional. If embeddings are unavailable, memory recall must degrade to FTS + entity + graph.

### `memory_edges`

Stores typed graph relationships.

```text
id
from_node_id
to_node_id
relation          # co_activated, predicts_success, predicts_failure, prefers, avoids, fails_due_to, validates_with, maintainer_style, agent_succeeds_on, agent_fails_on, repo_has_pattern
strength
confidence
evidence_event_ids_json
last_activated_at
created_at
updated_at
tombstoned_at
```

Edges are derived and may be rebuilt. They must preserve evidence event references.

### `memory_activation_runs`

One activation run per memory query.

```text
id
query_kind         # scout_ranking, dispatch_planning, github_draft, profile_review
query_ref          # issue key, run id, repo, profile, etc.
query_json
created_at
```

### `memory_activation_items`

Trace rows for surfaced memory nodes.

```text
run_id
node_id
source_channel     # fts, embedding, entity, recent, near_ripple, far_ripple
direct_score
ripple_score
salience_score
strength_score
recency_score
resource_penalty
hub_penalty
role_trust_penalty
hop_penalty
stale_penalty
final_score
rank
explanation
```

### `memory_writebacks`

Audit log for post-activation state updates.

```text
id
activation_run_id
node_id
action             # recalled, reinforced, edge_reinforced, resource_decremented
before_json
after_json
created_at
```

Write-back rows are required so memory learning can be debugged and evaluated.

### `memory_dream_runs`

Tracks consolidation jobs.

```text
id
trigger            # scheduled, manual, after_dispatch, after_feedback, after_eval, after_profile_bootstrap
scope              # global, repo, agent, profile, issue_type
input_activation_run_ids_json
model_status       # disabled, success, failed
created_at
```

### `memory_dreams`

Candidate summaries and synthesized observations.

```text
id
dream_run_id
dream_type         # profile_adjustment, repo_summary, agent_performance, discovery_policy, stale_memory, conflict
summary
evidence_node_ids_json
evidence_event_ids_json
evidence_hint_ids_json
status             # candidate, approved, rejected, stale, tombstoned
confidence
version
created_at
reviewed_at
```

Dreams are not stable policy. They are reviewable candidates.

### `memory_hints`

Decision-eligible memory only after approval.

```text
id
dream_id
hint_type          # ranking, dispatch, github_draft, profile_candidate
scope_type         # global, repo, agent, issue_type, maintainer
scope_ref
summary
policy_json
weight
status             # candidate, approved, rejected, pinned, deprioritized, suppressed, tombstoned
created_at
approved_at
expires_at
```

Only `approved` and `pinned` hints may affect ranking, dispatch, GitHub drafts, or profile suggestions.

## Recall And Activation

### Query Kinds

Memory is queried differently depending on use:

```text
scout_ranking
  issue title/body/labels/repo/profile terms

dispatch_planning
  issue package + agent id + repo + task category

github_draft
  repo + maintainer + prior comments + fix result

profile_review
  recent approvals/rejections/failures/successes
```

### Seed Channels

Activation starts from direct anchors:

```text
FTS
  Rare words, repo names, maintainer names, labels, error codes, validation commands.

Embedding
  Semantic similarity across issues, feedback, failure descriptions, and preference descriptions.

Entity match
  Structured matches for repo, agent, language, stack, issue_type, failure_reason, validation_path.

Recent context
  Recent feedback, dispatch outcomes, GitHub interactions, and profile bootstrap evidence.
```

Seed results are direct anchors. They do not become final memory context until score settlement.

### Ripple Channels

Graph ripple adds associative recall.

```text
near_ripple
  Bounded to same issue task, repo, dispatch run, or recent time window.
  Recovers the surrounding episode.

far_ripple
  Cross-graph one or two-hop expansion.
  Requires multiple seeds or strong trusted edges to enter the candidate pool.
```

Ripple may surface memories that are not semantically similar but are historically related.

### Score Settlement

Every activation run computes final score from multiple factors:

```text
final_score =
  direct_score
  + ripple_score
  + salience_score
  + strength_score
  + recency_score
  - resource_penalty
  - hub_penalty
  - role_trust_penalty
  - hop_penalty
  - stale_penalty
```

No field is the single definition of importance.

Factor meanings:

```text
direct_score
  Strength of FTS, embedding, rare-token, or entity match.

ripple_score
  Associative graph evidence.

salience_score
  Event-born significance, such as user rejection, dispatch success, dispatch failure, or maintainer reply.

strength_score
  Long-term usefulness from previous successful activations.

recency_score
  Recent evidence matters more for current recommendations, but does not automatically override strong long-term patterns.

resource_penalty
  Recently used memory steps back temporarily to avoid repetitive explanations.

hub_penalty
  Generic high-degree nodes such as "rust", "bug", or "good first issue" cannot dominate.

role_trust_penalty
  LLM-inferred memories are weaker than agent observations, GitHub facts, and user-explicit feedback.

hop_penalty
  Distant ripple needs direct or multi-seed support.

stale_penalty
  Expired, contradicted, suppressed, or tombstoned context is removed or heavily penalized.
```

### Activation Trace

Every memory use writes:

```text
memory_activation_runs
memory_activation_items
```

Exception: `memory off`, `no-write mode`, and `temporary mode` suppress activation/write-back persistence as described in Authority And Controls.

The trace must explain:

- Which memories surfaced.
- Which channel surfaced each item.
- Why a graph-related item appeared despite weak semantic similarity.
- Which items were penalized by resource, hub, trust, hop, or stale controls.
- Which approved hints are eligible to influence the current decision.

## Write-Back Guard

Activation may be broad. Write-back must be conservative.

A node may be reinforced only if:

```text
it is top activated
and has direct anchor or multi-seed support
and is not pure far-ripple
and is not a high hub
and is not LLM-only unsupported
and is not stale, suppressed, or tombstoned
```

A node may be shown as context but not reinforced if:

```text
it is pure ripple noise
or high hub
or stale/conflicting
or LLM-only unsupported
or outside trust threshold
```

Write-back actions:

```text
recalled
  Updates last_recalled_at and recall_count.

reinforced
  Increases strength and last_reinforced_at.

edge_reinforced
  Strengthens co_activated or predicts_* edges.

resource_decremented
  Spends short-term resource so recently used memories step back.
```

All write-back actions must write `memory_writebacks` with before/after state.

## Dreaming Consolidation

Dreaming is asynchronous or explicit. It is not required for `scout`, `assess`, `prepare`, or `daily`.

Triggers:

```text
scheduled
manual
after_dispatch_completed
after_feedback_batch
after_recommendation_eval
after_profile_bootstrap
```

Inputs:

```text
activation traces
recent raw events
high-salience events
conflicting hints
stale hints
agent performance changes
```

Dream types:

```text
profile_adjustment
  Candidate user preference or constraint.

repo_summary
  Contribution experience for a repository.

agent_performance
  Agent success/failure patterns by task type.

discovery_policy
  Candidate recommendation policy adjustment.

stale_memory
  Memory that may no longer be current.

conflict
  New behavior contradicts older hints.
```

Dreaming may use deterministic templates or optional LLM synthesis. LLM failure must not block memory operation. LLM output must be stored as candidate dream/hint only.

## Authority And Controls

User or deterministic policy can apply:

```text
approve
reject
pin
deprioritize
suppress
tombstone
restore prior version
memory off
no-write mode
```

State rules:

```text
candidate
  Visible for review, cannot affect decisions.

approved
  Can affect decisions.

pinned
  Can affect decisions and resists resource suppression, but not tombstone.

deprioritized
  Can affect decisions with lower weight.

suppressed
  Hidden for the scope, cannot affect decisions.

rejected
  Cannot affect decisions.

stale
  Cannot affect decisions unless renewed.

tombstoned
  Cannot be recalled, indexed, dreamed, hinted, or used.
```

Controls:

```text
memory off
  Memory returns no decision hints and does not write activation/write-back.

no-write mode
  Memory can recall for display but does not write activation/write-back or dreams.

temporary mode
  Equivalent to no-write plus no raw event ingestion for that operation.
```

Deletion/tombstone semantics are stronger than natural decay. Tombstoned raw events must invalidate derived nodes, indexes, edges, dreams, and hints.

## Integration With Existing Systems

### Recommendation Ranking

Ranking may consume:

```text
approved ranking hints
pinned ranking hints
activation trace explanations
```

Ranking must not consume:

```text
candidate dreams
rejected hints
LLM-only unsupported claims
raw private payloads
```

Memory can influence:

```text
profile fit adjustment
repo trust adjustment
issue type preference or avoidance
scope/risk signal
explanation text
```

Memory must not duplicate prepare gate rules. `prepare_gate.rs` remains the single prepare gate implementation.

Any change to ranking, freshness, feedback, quality policy, source trust, or fallback behavior must update recommendation eval fixtures or document why no fixture change is required.

### Dispatch

Dispatch may consume:

```text
agent_performance hints
repo validation_path hints
failure_reason hints
task_type success/failure hints
```

Examples:

```text
Codex succeeded on similar Rust CLI panic fixes.
Broad frontend refactors have failed due to unclear scope.
This repo usually validates parser changes with cargo test -p parser.
```

Dispatch should add relevant memory context to `IssueTaskPackage.memory_context`:

```text
approved_hints
activation_run_id
evidence_refs
risk_notes
agent_selection_notes
```

### GitHub Drafts

GitHub drafts may consume:

```text
maintainer_style hints
repo_interaction_style hints
approved wording preferences
previous successful final comment style
```

GitHub drafts must not expose private raw payload. Any memory-derived wording must be explainable and sanitized.

### Profile Learning

Dreaming may create:

```text
profile_adjustment_candidate
```

Applying profile changes remains explicit:

```text
issue-finder profile suggestions
issue-finder profile apply-suggestion <hint-id>
```

No memory dream directly edits `config.toml`.

### Offline Mode

Memory must degrade cleanly:

```text
memory unavailable -> no memory hints
embedding unavailable -> FTS + entity + graph
LLM unavailable -> deterministic dreams or no dreams
dispatch unavailable -> recommendation memory still works
GitHub write unavailable -> drafts only or disabled
```

Existing offline commands remain independent:

```text
issue-finder scout
issue-finder assess
issue-finder prepare
issue-finder daily
issue-finder eval recommendation --offline
```

## Module Boundary

Recommended modules:

```text
src/memory/mod.rs
src/memory/model.rs
src/memory/store.rs
src/memory/ingest.rs
src/memory/index.rs
src/memory/graph.rs
src/memory/activation.rs
src/memory/writeback.rs
src/memory/dreaming.rs
src/memory/hints.rs
src/memory/eval.rs
```

`dispatch::store` remains responsible for dispatch runtime tables. `memory::store` owns memory tables and APIs. They may share the same SQLite database file, but should not share giant store methods in one file.

## CLI

User commands:

```bash
issue-finder memory status
issue-finder memory events --issue owner/repo#123
issue-finder memory recall --issue owner/repo#123 --kind scout-ranking
issue-finder memory dreams list
issue-finder memory dreams show <dream-id>
issue-finder memory hints list
issue-finder memory hints approve <hint-id>
issue-finder memory hints reject <hint-id>
issue-finder memory hints pin <hint-id>
issue-finder memory hints deprioritize <hint-id>
issue-finder memory suppress --scope repo:owner/repo
issue-finder memory tombstone <event-or-node-id>
issue-finder memory dream --scope global
issue-finder memory eval --offline --output <dir>
```

Human output should be compact. JSON output should be available for agent consumption where useful.

## JSON Tool Contract

New tools:

```text
issue-finder.memory_status
issue-finder.memory_recall
issue-finder.memory_dreams_list
issue-finder.memory_dream_show
issue-finder.memory_hints_list
issue-finder.memory_hint_update
issue-finder.memory_tombstone
```

Tool output rules:

- stdout remains a single JSON object.
- Large raw payloads are not returned by default.
- Invalid transitions return structured failures.
- Business states such as no memory, no embedding, or no approved hints are not system errors.

Example recall output:

```json
{
  "activationRunId": "memory-activation-1",
  "queryKind": "scout_ranking",
  "items": [
    {
      "rank": 1,
      "nodeId": "memory-node-1",
      "eventId": "memory-event-1",
      "summary": "User rejected broad feature requests with unclear validation.",
      "sourceChannel": "entity+near_ripple",
      "scores": {
        "direct": 0.4,
        "ripple": 0.8,
        "salience": 0.7,
        "strength": 0.6,
        "resourcePenalty": 0.1,
        "hubPenalty": 0.0,
        "final": 1.9
      },
      "explanation": "Matched issue_type:broad_feature and prior rejection events in the same repo family.",
      "evidenceRefs": ["memory-event-1"]
    }
  ],
  "decisionEligibleHints": []
}
```

Allowed hint transitions:

```text
candidate -> approved
candidate -> rejected
approved/pinned/deprioritized/suppressed -> approved
approved/pinned/deprioritized/suppressed -> pinned
approved/pinned/deprioritized/suppressed -> deprioritized
approved/pinned/deprioritized/suppressed -> suppressed
candidate/approved/pinned/deprioritized/suppressed -> prior version through restore, if audit history exists
approved/pinned/deprioritized/suppressed -> stale
approved/pinned/deprioritized/suppressed -> tombstoned
```

## Implementation Milestones

### Milestone 1: Memory Schema + Store

Implement all memory tables and typed models.

Acceptance:

```text
schema initialization is idempotent
raw event insert/read/list works
node + state insert/read works
edge insert/update works
activation run/item insert works
dream/hint insert and state transition works
tombstone event/node/hint works
all tests use temporary IssueFinderPaths
```

### Milestone 2: Ingestion Pipeline

Ingest from existing systems.

Inputs:

```text
recommendation events
dispatch events
agent artifacts metadata
GitHub interactions metadata
profile bootstrap evidence
manual memory events
```

Acceptance:

```text
recommendation feedback -> raw events + nodes
dispatch success/failure -> raw events + agent/task/failure nodes
profile bootstrap evidence -> raw events with source refs
role/trust/source_ref preserved
duplicate ingest is idempotent
```

### Milestone 3: Index + Graph Builder

Implement FTS/entity/rare-token indexes, optional embedding abstraction, and graph edge builder.

Acceptance:

```text
FTS recalls rare repo/label/error tokens
entity match recalls repo/agent/failure_reason
embedding disabled gracefully
graph edges can be rebuilt from raw events
tombstoned events do not enter rebuilt index/graph
```

### Milestone 4: Activation Engine

Implement seed, ripple, settlement, and trace persistence.

Acceptance:

```text
semantic direct match works
rare-token match works
graph-only related node can surface through ripple
hub nodes are penalized
recently used nodes get resource penalty
LLM-only inferred nodes cannot dominate user-explicit feedback
activation trace explains score components
```

### Milestone 5: Write-Back Guard

Implement conservative reinforcement.

Acceptance:

```text
good graph recall reinforces strength/edge
pure ripple noise is shown but not reinforced
resource is decremented after recall
tombstoned/suppressed nodes never reinforce
writebacks include before/after state
```

### Milestone 6: Dreaming Consolidation

Implement deterministic and optional LLM dream generation.

Acceptance:

```text
dream consumes activation traces and raw evidence
dream creates candidate dreams/hints only
dream stores evidence node/event ids
stale/conflict candidates generated when evidence contradicts old hints
LLM disabled path works
```

### Milestone 7: Authority + Controls

Implement review and control state transitions.

Acceptance:

```text
candidate hint cannot affect decisions
approved hint can be queried as decision-eligible
pinned hint survives resource suppression but not tombstone
deprioritized hint lowers weight
suppressed scope hides hints from that scope
tombstone invalidates raw event, nodes, activations, dreams, and hints
memory off returns no hints
no-write mode recalls but does not write back
```

### Milestone 8: CLI + Tool Contract

Implement memory CLI and JSON tools.

Acceptance:

```text
CLI human output is compact and clear
JSON/tool output is a single JSON object
large raw payloads are not printed by default
invalid state transitions fail structurally
```

### Milestone 9: Consumption Integration

Integrate approved hints into ranking, dispatch, IssueTaskPackage, GitHub drafts, and profile suggestions.

Acceptance:

```text
memory unavailable -> no-op fallback
embedding unavailable -> FTS/entity fallback
LLM unavailable -> no dream or deterministic dream
ranking explanation includes memory hint refs
candidate dreams do not affect ranking
recommendation eval updated if ranking behavior changes
```

### Milestone 10: Memory Eval

Add offline memory eval.

Eval dimensions:

```text
factual recall
preference following
stale correction
over-recall prevention
deletion/tombstone
graph value
anti-hub
write-back safety
```

Acceptance:

```text
fixture-only tests
no real GitHub/Codex/LLM/network
metrics.json and report.md output
failing samples explain expected behavior
```

## Risk Controls

- Do not let LLM write stable truth.
- Do not merge user/system/agent/github/llm roles.
- Do not store private raw payload in GitHub drafts.
- Do not make memory mandatory for offline scout/assess/prepare.
- Do not duplicate prepare gate rules inside memory.
- Do not let generic high-degree nodes dominate explanations.
- Do not reinforce pure ripple noise.
- Do not leave tombstoned data active in index, graph, dreams, or hints.
- Do not expose raw event payloads through tool output unless explicitly requested and safe.

## Testing

Required test categories:

```text
store/schema tests
ingestion tests
index rebuild tests
activation scoring tests
write-back guard tests
dreaming tests
authority/control tests
tool contract tests
ranking integration tests
dispatch integration tests
memory eval tests
```

All tests must use fixtures, temp state, or mock services. No automated test may require real GitHub, real Codex, real LLM services, user state, or external network.

## Documentation Updates

When implementing, update:

```text
README.md
README.zh-CN.md
docs/usage.md
docs/issue-finder-rust-design.md
docs/agent-safe-preparation-runtime.md
the dispatch control-plane spec, if one exists when memory changes dispatch assumptions
```

The docs should explain:

- memory is optional
- raw events are truth
- dreams are candidates
- approved hints affect decisions
- users can inspect, approve, reject, pin, deprioritize, suppress, and tombstone memory
- offline mode remains independent

## Final Acceptance Criteria

```text
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets -- -D warnings

Existing scout/assess/prepare/handoff/daily/profile bootstrap/tool contract tests pass.
Offline recommendation eval still works without memory.
Memory eval works without external services.
Memory controls can show, approve, reject, pin, deprioritize, suppress, and tombstone.
Every ranking/dispatch/GitHub use of memory is explainable by evidence refs.
LLM-generated dreams never directly affect decisions without approval.
Tombstoned raw events no longer influence indexes, graph activation, dreams, or hints.
```
