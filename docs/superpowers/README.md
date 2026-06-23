# Superpowers Design Archive

The Markdown files under `docs/superpowers/specs/` are a chronological record of design decisions, trade-offs, and implementation plans. Treat them as historical context, not as the primary reference for current behavior.

For current contributor guidance, prefer:

- [`AGENTS.md`](../../AGENTS.md) for repository collaboration rules and owner boundaries.
- [`docs/usage.md`](../usage.md) for the current CLI, local state layout, and dispatch workflow.
- [`docs/agent-safe-preparation-runtime.md`](../agent-safe-preparation-runtime.md), [`docs/sandbox.md`](../sandbox.md), and [`docs/execpolicy.md`](../execpolicy.md) for the current preparation and safety boundary.
- Source owner modules for executable truth: `src/prepare_gate.rs`, `src/dispatch/*`, `src/memory/*`, `src/recommendation/*`, `src/tool_specs.rs`, and `src/tool_runtime.rs`.

When a spec conflicts with source code or the current usage guide, use the current implementation and owner docs. Do not update old spec files just to remove drift; add or update current-facing docs instead.
