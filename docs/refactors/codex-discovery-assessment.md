# Codex discovery and assessment contract

## Scope and implementation plan

The Codex session contract moves from version 1 to version 2. Only `scout` and
`assess` remain, and both JSON tools and MCP default to that profile. Codex owns
selection, workspace preparation, reproduction, repair, validation, review, and
PR delivery. Cloud configuration owns installation, dependencies, PATH, and
supported authentication setup.

1. Reduce the catalog and executable allowlist together; report configuration,
   authentication, and network errors on business calls.
2. Remove session task creation, identity/baseline tracking, locks, check runners,
   completion records, and their dedicated workspace/path helpers.
3. Make scores and missing evidence advisory. Remove session `prepareGate` and
   bypass requirements rather than moving them into the remaining tools.
4. Read only shown/read feedback for Codex ranking. Ignore historical dismissed,
   done, restored, and prepared events non-destructively; retain existing files.
5. Reduce the CLI Skill to invocation rules, parameters, evidence interpretation,
   and refresh rules, and align current documentation and default adapters.
6. Verify meaningful contract boundaries, old-state visibility, and assessment
   side effects; then run formatting, clippy, and the repository test suite.

## Compatibility and removed guarantees

Legacy control/worker tools, terminal commands, dispatch, handoff, and the
independent Python skill still have callers of shared modules. They remain
explicit compatibility paths outside the Codex session contract. Removing their
entire product surface would be a separate migration. The default Codex tools
never fall back to them.

Old session task files remain on disk but are no longer read or resumed. Codex
has no CLI-specific durable execution baseline, check/result log, or manual
cross-chat dismiss/restore entry point. Existing recommendation event files are
not rewritten. Automatic shown/read history still affects repeat ranking.

## Case evidence

The referenced 0930研发中控 chat was read before implementation. Its recap shows
that `octop-harness#24` was reproducible despite low-star recommendation gating,
but additional investigation found three competing PRs. Java SDK `#286` was
reproduced and fixed using general tools; a successful push did not establish
permission to create an upstream PR. These cases justify separating advisory
recommendation evidence from execution authorization and delivery status.

This change does not resolve competing-PR detection gaps, GitHub rate limiting,
or Cloud authentication routing. Partial evidence remains partial. The retained
tools still need evaluation for recommendation quality, evidence completeness,
and investigation time saved.

## Validation rationale

Retain existing discovery/assessment tests and remove tests exclusively for the
deleted lifecycle. Add regression coverage only for consequential boundaries:
removed tools cannot execute, default adapters expose the intended tools, errors
remain structured even with invalid configuration, and old terminal feedback
cannot silently hide Codex candidates. Legacy fixture behavior remains covered
separately because legacy callers still exist.

## Implementation and verification

Completed in the working tree. The CLI Skill now has 105 lines, with two
references totaling 145 lines. The requested collaboration instructions and
`gpt-6.1-sol` / `high` subagent choice are recorded verbatim in the root guide
(for the English instructions) and as the explicit model policy.

Validation passed: `cargo fmt --all -- --check`,
`cargo clippy --all-targets -- -D warnings`, the complete `cargo test` suite,
and all 22 standalone Python compatibility tests. A separate read-only review
found no substantive regression. No live GitHub recommendation-quality or
publication-permission evaluation was performed as part of these offline checks.
