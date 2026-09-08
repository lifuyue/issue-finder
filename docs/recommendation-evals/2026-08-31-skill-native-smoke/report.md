# Standalone Skill smoke check — 2026-08-31

Scope: one public repository, read-only GitHub discovery, no target clone, edits,
commit, or GitHub write. This is not the fixed six-profile evaluation or a real
Codex implementation run, and does not establish parity with the Rust ranking engine.

```bash
python3 skills/issue-finder/scripts/issue_finder.py scout \
  --repo rust-lang/cargo --limit 2 --scan-limit 5 --json
```

The final run scanned five issues, returned one candidate, rejected four, and
reported no GitHub evidence warnings. Exit status was 0.

- [Cargo #17182](https://github.com/rust-lang/cargo/issues/17182) initially passed
  the coarse filter despite `S-blocked-external`. The visible maintainer discussion
  explicitly requires stabilization and a waiting period before cleanup. Added
  blocked-label filtering and the `blocked_external_cleanup` offline case.
- [Cargo #13256](https://github.com/rust-lang/cargo/issues/13256) is an automated
  dependency dashboard, not a bounded implementation issue. Added the
  `dependency_dashboard` regression case and excluded that explicit title.
- [Cargo #13290](https://github.com/rust-lang/cargo/issues/13290) remained visible
  with score 45 and a tracking-issue scope warning. The returned body and recent
  discussion show unresolved prerelease behavior and compatibility considerations.
  It is evidence for further Codex review, not a validated selection or a claim
  that this broad tracking issue can be completed in one contribution.
- #13136 and #13259 were excluded because they had assignees.

Validation: 22 Python offline integration tests passed, including 11 coarse-filter
fixtures and a copied-package CLI flow with a mocked GitHub executable. The existing
Rust suite, including its offline recommendation eval, passed (`cargo test --offline`;
three real-daemon tests remained intentionally ignored). Clippy and rustfmt checks
passed. The initial sandbox blocked local mock listeners and GitHub networking;
the same checks succeeded after host permission review, without changing test logic.

Only reduced observations are retained here. No credentials, raw GitHub response
cache, generated user state, or contribution workspace is included.
