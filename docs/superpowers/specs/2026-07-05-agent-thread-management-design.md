# Agent Thread Management Design

Date: 2026-07-05

## Goal

Issue Finder's online agent daemon must behave like a resumable agent thread, not a one-shot task runner. A caller should be able to create a long-lived thread, append later turns, inspect events and transcript items, and let the Issue Finder LLM choose safe Issue Finder tools inside each turn.

## Design

The daemon keeps its own local A2A control plane:

- `POST /a2a/threads/start` creates an Issue Finder thread and first turn.
- `POST /a2a/threads/{threadId}/turns/start` appends a new user turn to an existing thread.
- `GET /a2a/threads`, `GET /a2a/threads/{threadId}`, and `GET /a2a/threads/{threadId}/events` expose state, transcript items, tool logs, and final results.
- Existing `/a2a/tasks/send` remains as a compatibility shim that creates a thread plus first turn and mirrors the terminal result back to the legacy task row.

SQLite becomes the context owner for the daemon. `agent_threads` is the long-lived session, `agent_turns` is one user-driven execution, `agent_thread_items` stores user messages, assistant decisions, tool calls, tool results, and final answers, and `agent_thread_events` stores the pollable A2A event stream. A turn can complete while the thread returns to `idle`, so later turns can continue with the previous transcript.

The external caller chooses control flow: start a new thread or continue an existing one. The internal Issue Finder LLM chooses business tools only inside a running turn. The default tool policy allows safe Issue Finder operations such as status, scout, assess, prepare, read-context, memory recall, and GitHub comment drafting/listing. Posting comments remains protected by the existing explicit GitHub approval/posting flow and is not automatically exposed to the daemon loop.

Codex native `thread/start` and `turn/start` remain a dispatch adapter for running Codex as an execution agent. The Issue Finder daemon does not use Codex as its storage layer. If a caller needs results in a Codex thread, it can call the Issue Finder A2A thread endpoints and relay the returned turn result; a later bridge can add an explicit `replyTo.codexThreadId` callback.

## What This Change Should Deliver

- A working thread/turn API and CLI surface.
- SQLite-backed context reconstruction before every LLM turn.
- Tool call and observation logs persisted as transcript items.
- Compatibility for old `agent send/list/show/events`.
- Updated docs that explain what exists and what remains future work.

## Non-Goals

- No automatic GitHub comment posting by the daemon LLM.
- No public-network A2A federation.
- No replacement of dispatch's existing Codex native session adapter.
- No target repository source edits, commits, pushes, or PR creation.

## Implementation Notes

Completed in the implementation branch:

- Added SQLite thread management tables for `agent_threads`, `agent_turns`, `agent_thread_items`, and `agent_thread_events`.
- Added local A2A thread endpoints for starting threads, appending turns, reading thread/turn detail, and polling thread events.
- Kept `/a2a/tasks/send` as a compatibility shim that creates a thread plus first turn and mirrors the final result to the legacy task row.
- Reworked the LLM loop to run per turn and rebuild context from persisted thread items before asking the model for the next tool or final answer.
- Expanded the safe daemon tool policy beyond status/scout to include assess, prepare, read-context, memory recall/status, dispatch inspection, and GitHub comment draft/list tools.
- Replaced the hand-written daemon prompt tool list with a Codex-style compact catalog generated from canonical Issue Finder `tool_specs`, so the LLM sees exact allowed tool names, descriptions, JSON input schemas, missing-argument behavior, and safety boundaries from the same allowlist used by runtime validation and the agent card.
- Added CLI commands for `agent thread-start`, `agent turn`, `agent threads`, `agent thread`, and `agent thread-events`.
- Verified a real two-turn LLM run where the second turn used the previous turn transcript without opening a new task.

Still intentionally not implemented:

- No SSE/event streaming; callers poll events and detail endpoints.
- No automatic callback into a Codex thread. Codex or another caller reads the A2A result and can relay it itself.
- No daemon-side approval or posting of GitHub comments.
