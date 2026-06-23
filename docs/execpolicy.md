# Execution Policy

Issue Finder execution policy is a handoff manifest, not an executor. `agent-policy.json` describes what downstream agents may consider safe, what needs user approval, and what is outside the prepared handoff boundary. Dispatch execution is a separate approval-gated control-plane path; native agent runtimes still enforce their own sandbox and approval behavior.

## Manifest Shape

```json
{
  "version": 1,
  "kind": "issue_finder_agent_policy",
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

- `read_roots`: the prepared workspace and Issue Finder inbox item.
- `write_roots`: the prepared workspace.
- `protected_roots`: workspace metadata and Issue Finder-generated handoff files.

`write_roots` is descriptive. It tells a downstream agent where task work belongs, but Issue Finder itself does not edit target source.

## Network Policy

Network is always represented as `requires_user_approval`. Issue Finder does not ask downstream agents to perform networked setup or validation automatically.

## Command Policy

Allowed low-risk commands are concrete argv arrays built by Issue Finder, for example:

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

Forbidden entries are pattern-level boundaries such as dependency installation, project-defined scripts, commits, pushes, pull requests, Issue Finder inbox edits, and destructive filesystem changes.
