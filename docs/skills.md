# Skills and Context Pack

Patchbay writes a small local skill and progressive context pack for coding agents that support skill-style handoffs.

## Generated Skill

Each prepared item includes:

```text
.agents/
  skills/
    patchbay-cli/
      SKILL.md
      refs.json
```

The generated skill tells the agent how to consume a Patchbay handoff:

1. Read `context/entry.md` and `context/safety.md` first.
2. Read `context/probe.md` before deciding which commands to run.
3. Defer value, issue, repo, and validation detail until needed.
4. Treat Patchbay inbox files as generated context, not target source.

## Codex Entry

`codex.md` is the shortest entrypoint. It includes absolute paths to:

- The handoff pack directory
- `handoff.json`
- `handoff.md`
- `agent-policy.json`
- `probe.json`
- The generated `patchbay-cli` skill
- Default context files

This keeps the entry usable even when the agent starts from the target workspace instead of the Patchbay inbox directory.

## Context Files

Default-visible context:

- `context/entry.md`
- `context/safety.md`
- `context/probe.md`

Deferred context:

- `context/value.md`
- `context/issue.md`
- `context/repo.md`
- `context/validation.md`

This keeps the first agent turn small while preserving complete handoff detail for later phases.
