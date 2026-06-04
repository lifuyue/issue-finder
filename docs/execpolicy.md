# Execution Policy

Patchbay execution policy is a manifest, not an executor. `agent-policy.json` describes what downstream agents may consider safe, what needs user approval, and what is outside the handoff boundary.

## Manifest Shape

```json
{
  "version": 1,
  "kind": "patchbay_agent_policy",
  "handoff_id": "2026-06-04-owner__repo-123",
  "permission_profile": {
    "filesystem": {
      "read_roots": [],
      "write_roots": [],
      "protected_roots": []
    },
    "network": "requires_user_approval"
  },
  "commands": {
    "allowed_low_risk": [],
    "requires_user_approval": [],
    "forbidden": []
  },
  "agent_constraints": []
}
```

## Filesystem Policy

- `read_roots`: the prepared workspace and Patchbay inbox item.
- `write_roots`: the prepared workspace.
- `protected_roots`: workspace metadata and Patchbay-generated handoff files.

`write_roots` is descriptive. It tells a downstream agent where task work belongs, but Patchbay itself does not edit target source during prepare.

## Network Policy

Network is always represented as `requires_user_approval`. Patchbay does not ask downstream agents to perform networked setup or validation automatically.

## Command Policy

Allowed low-risk commands are concrete argv arrays built by Patchbay, for example:

```json
{
  "argv": ["git", "status", "--porcelain"],
  "cwd": "/abs/path/to/workspace",
  "reason": "Read workspace dirty state."
}
```

Approval-required commands are plain command strings from repository detection, for example:

```json
{
  "command": "cargo test",
  "reason": "Detected Cargo.toml; detected validation may execute repository code"
}
```

Forbidden entries are pattern-level boundaries such as dependency installation, project-defined scripts, commits, pushes, pull requests, Patchbay inbox edits, and destructive filesystem changes.
