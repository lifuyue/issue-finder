# Agent-Safe Preparation Runtime

Patchbay prepares coding-agent handoffs without becoming a coding agent itself. The agent-safe preparation runtime adds structured safety, probe, and audit artifacts to each prepared inbox item so Codex, Cursor, Claude Code, or a human developer can start from a clearer boundary.

Patchbay still does not modify target repository source, install dependencies, run full validation, commit, push, or create pull requests.

## What It Writes

Each successful `patchbay prepare` or selected `patchbay daily` item writes:

```text
inbox/<id>/
  handoff.json
  handoff.md
  codex.md
  agent-policy.json
  probe.json
  prepare-events.jsonl
  context/
    entry.md
    safety.md
    probe.md
    value.md
    issue.md
    repo.md
    validation.md
  .agents/
    skills/
      patchbay-cli/
        SKILL.md
        refs.json
```

`handoff.json` remains canonical. The new runtime fields are additive:

- `agent_policy`: same content as `agent-policy.json`
- `probe_pack`: same content as `probe.json`
- `readiness`: preparation-focused readiness score

## Runtime Flow

```text
prepare owner/repo#123
  -> fetch issue metadata
  -> prepare local workspace and Patchbay branch
  -> scan repository structure
  -> run fixed safe probes
  -> classify validation commands
  -> compute preparation readiness
  -> write handoff, policy, probe, event, and context artifacts
  -> upsert the inbox item
```

Probe failures are recorded as warnings by default. Missing binaries, non-zero exits, timeouts, truncation, and invalid UTF-8 do not fail prepare unless a required safety artifact cannot be written.

## Topic Docs

- [Sandbox & approvals](./sandbox.md)
- [Execution policy](./execpolicy.md)
- [Safe probes](./safe-probes.md)
- [Skills and context pack](./skills.md)

## Downstream Agent Use

Give `codex.md` to a coding agent first. It points to the minimal default context:

- `context/entry.md`
- `context/safety.md`
- `context/probe.md`

The agent should load `context/value.md`, `context/issue.md`, `context/repo.md`, and `context/validation.md` only when that phase needs the detail.
