# Tool Spec Onboarding Metadata Design

## Context

`issue-finder tools list` currently looks like a low-level API catalog. The envelope exposes `kind`, `version`, and `tools`, while each tool has only a short description, input schema, and deferred-loading flag. That is enough for an adapter that already knows Issue Finder, but it is weak as a first-contact entry point for a coding agent.

The agent-facing workflow is ordered:

1. Discover candidates with `issue-finder.scout`, usually scoped by `repo` when the user names a repository.
2. Assess the top candidate with `issue-finder.assess`.
3. Prepare the issue with `issue-finder.prepare` if the prepare gate allows it.
4. Read deferred handoff context with `issue-finder.read_context`, starting with `entry`, `safety`, and `probe`.

The current contract makes agents infer that sequence from individual descriptions. The contract should expose the workflow directly while keeping progressive disclosure intact.

## Goals

- Make `tools list` read as an agent workflow entry point, not only an API list.
- Add top-level onboarding metadata: `quickStart`, `firstCall`, and `recommendedWorkflow`.
- Keep `read_context` deferred and avoid inlining large handoff context into `tools list`.
- Split tool contract catalog responsibilities out of `tool_runtime.rs`.
- Preserve existing tool call behavior and runtime error semantics.

## Non-Goals

- No new tool names.
- No change to `tools call` invocation or output shape.
- No change to scout ranking, fallback, freshness, feedback cooldown, quality policy, or prepare gate behavior.
- No MCP adapter or `tool_search` integration in this change.
- No human-formatted `tools list` table; the command remains single-object JSON for agent consumption.

## Architecture

Create a dedicated contract catalog module, tentatively `src/tool_specs.rs`.

This module owns:

- `IssueFinderToolSpecsEnvelope`
- `IssueFinderToolSpec`
- `ToolQuickStart`
- `ToolFirstCall`
- `ToolWorkflowStep`
- input schema builders
- `list_tool_specs()`

`src/tool_runtime.rs` should keep runtime responsibilities only:

- `IssueFinderToolInvocation`
- `IssueFinderToolOutput`
- dispatch
- concrete tool call implementations
- runtime error mapping

`src/main.rs` and contract tests should import `list_tool_specs()` from the new contract catalog module. Runtime code should not need to construct the tool list.

This boundary keeps discovery metadata separate from execution. `quickStart`, `firstCall`, and `recommendedWorkflow` are not runtime behavior; they are catalog metadata for agents and adapters. The split also leaves a cleaner place for future MCP adapter metadata, `tool_search` hints, or richer deferred-loading descriptions.

## Envelope Shape

The envelope remains a single JSON object:

```json
{
  "kind": "issue_finder_tool_specs",
  "version": 1,
  "quickStart": {
    "summary": "Use scout to find candidates, assess the top issue, prepare it if the gate allows, then read deferred context sections as needed.",
    "firstCall": {
      "defaultTool": "issue-finder.scout",
      "defaultArguments": {
        "repo": "owner/repo",
        "limit": 10
      },
      "whenReadyUnknown": "issue-finder.status",
      "fallbackAfterSetupFailure": "issue-finder.status"
    }
  },
  "recommendedWorkflow": [
    {
      "step": "discover",
      "tool": "issue-finder.scout",
      "purpose": "Find and rank candidates. Use repo when the user named a repository."
    },
    {
      "step": "assess",
      "tool": "issue-finder.assess",
      "purpose": "Assess the best candidate before preparing workspace state."
    },
    {
      "step": "prepare",
      "tool": "issue-finder.prepare",
      "purpose": "Prepare workspace and handoff only when the prepare gate allows."
    },
    {
      "step": "read_context",
      "tool": "issue-finder.read_context",
      "purpose": "After prepare, read entry first, then safety and probe; read larger sections only when needed.",
      "deferred": true,
      "firstSections": ["entry", "safety", "probe"]
    }
  ],
  "tools": []
}
```

`version` can remain `1` because these are additive top-level fields. Existing consumers that ignore unknown fields continue to work. If an adapter treats the envelope as closed over exactly three top-level keys, that adapter is already incompatible with additive contract evolution and should be updated.

## Data Flow

`tools list`:

1. `main.rs` handles `issue-finder tools list`.
2. It calls `tool_specs::list_tool_specs()`.
3. `tool_specs` builds the static contract catalog and onboarding metadata.
4. `main.rs` serializes the envelope as one JSON object.

`tools call`:

1. `main.rs` parses the requested tool name and arguments.
2. It constructs `IssueFinderToolInvocation`.
3. `IssueFinderToolRuntime::call()` dispatches to runtime handlers.
4. Runtime returns `IssueFinderToolOutput`.

The catalog module must be pure construction. It should not load config, read local state, create paths, call GitHub, or instantiate `IssueFinderToolRuntime`.

## Error Handling

This change does not add runtime error paths.

`firstCall` is guidance, not a dispatcher rule. The runtime does not automatically call `status`, retry `scout`, or alter arguments based on this metadata.

`recommendedWorkflow` is guidance, not a guarantee. `prepare` can still return `blocked_by_gate` or `prepare_failed`; agents should then return to `assess` or `scout` and choose another candidate.

`tools list` remains infallible construction plus JSON serialization through the existing CLI error handling.

## Testing

Update `tests/tools_contract.rs` so `tools_list_outputs_stable_issue_finder_specs` verifies that `tools list` is an agent workflow entry point:

- top-level `kind` and `version` remain stable.
- top-level `quickStart` exists.
- `quickStart.firstCall.defaultTool` is `issue-finder.scout`.
- `quickStart.firstCall.whenReadyUnknown` is `issue-finder.status`.
- `quickStart.firstCall.fallbackAfterSetupFailure` is `issue-finder.status`.
- `recommendedWorkflow` order is `issue-finder.scout`, `issue-finder.assess`, `issue-finder.prepare`, `issue-finder.read_context`.
- the `read_context` workflow step has `deferred: true`.
- the `read_context` workflow step recommends `entry`, `safety`, and `probe` as first sections.
- the five existing tool specs remain present.
- each tool still has an object `inputSchema`.
- `issue-finder.read_context` still has `deferLoading: true`.

Add or update a CLI adapter test if needed so `issue-finder tools list` still prints one JSON object and includes the onboarding metadata.

No recommendation eval fixture is required because the change does not alter discovery, ranking, fallback, feed ranking, quality policy, freshness, or feedback cooldown.

## Documentation

Update `docs/superpowers/specs/2026-06-04-codex-tool-contract-design.md` so it no longer describes the envelope as only `kind`, `version`, and `tools`, and so the old struct sketch points readers to the split contract catalog module.

## Acceptance Criteria

- `src/tool_specs.rs` owns tool spec DTOs, schema builders, onboarding DTOs, and `list_tool_specs()`.
- `src/tool_runtime.rs` no longer owns tool spec catalog construction.
- `issue-finder tools list` returns the additive onboarding metadata.
- Runtime behavior for all existing tools is unchanged.
- Contract tests cover onboarding metadata and existing tool specs.
- Existing docs no longer conflict with the new envelope shape.
