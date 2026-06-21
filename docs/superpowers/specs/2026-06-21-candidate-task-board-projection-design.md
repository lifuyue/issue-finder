# Candidate Task Board Projection Design

Date: 2026-06-21

Status: Implemented in `fuyue/candidate-task-board-projection`

## Summary

The candidate task board is a derived read model over existing Issue Finder state. It gives later workflow, daily, prepare, dispatch, and product-surface code one query surface for the local candidate lifecycle without creating a new source of truth.

The board does not persist a materialized table. It replays local facts from:

- recommendation events in `recommendation/events.jsonl`
- inbox entries in `inbox/index.json`
- dispatch issue tasks, review approvals, packages, runs, and outcomes in `dispatch/dispatch.sqlite3`

Owner modules remain authoritative for writes and policy.

## Lifecycle

The projection emits one `CandidateLifecycleStatus` per issue:

```text
discovered
ranked
prepared
review_pending
package_ready
dispatch_running
outcome_positive
outcome_negative
snoozed
reactivation_candidate
archived
```

Status is paired with a display state:

```text
visible
hidden_snoozed
hidden_archived
```

Keeping status and display separate is intentional. A dismissed or archived task can be hidden from active board views without erasing a terminal dispatch result.

## Precedence

Projection precedence is:

1. Dispatch outcome, if present.
2. Terminal dispatch run status, when no outcome was recorded.
3. Active dispatch run.
4. Reactivation candidate.
5. Archived or snoozed display state.
6. Pending issue review.
7. Package ready.
8. Prepared.
9. Ranked.
10. Discovered.

Dispatch terminal outcome wins over inbox `done`. Dismissed and archived states affect display, but they do not override an existing terminal outcome. Reactivation is projected as local board state only; it does not change feed score, freshness, feedback cooldown, memory hints, or recommendation eval behavior.

## Components

- `src/candidate_board.rs` owns DTOs, projection replay, and query helpers.
- `DispatchStore` exposes minimal read helpers for listing issue tasks and dispatch runs by issue task.
- `src/recommendation/*`, `src/inbox.rs`, `src/dispatch/*`, and `src/memory/*` continue to own their respective write paths and policy.

`TaskBoard` exposes focused queries such as `active`, `ready_for_review`, `ready_for_dispatch`, and `reactivation_candidates`. These are library APIs first; no CLI or JSON tool contract is required for the first board implementation.

## Non-Goals

- Do not add a materialized task board table.
- Do not duplicate the dispatch state machine.
- Do not copy prepare gate policy.
- Do not change recommendation ranking or memory ranking adjustments.
- Do not add a complex UI.

## Testing

Integration tests cover:

- review, package-ready, and dispatch-running projection
- dispatch terminal outcome priority over inbox done/archive display
- snooze and reactivation projection from recommendation event replay

Because this branch does not alter discovery, fallback, feed ranking, quality policy, freshness, feedback cooldown, or memory ranking behavior, recommendation eval fixtures do not need new samples.
