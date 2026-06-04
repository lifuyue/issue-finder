# Agent-Safe Preparation Runtime 设计

日期：2026-06-04

状态：已确认设计方向

## 摘要

Patchbay 的下一阶段应升级为 **Agent-Safe Preparation Runtime**：一个纯算法、受控探测、面向 coding agent 协同的准备层。

Patchbay 仍然不是通用 coding agent，不直接接入 LLM，不生成 patch，不修改目标仓库源码，不运行完整验证矩阵，不提交、不推送、不创建 PR。它的职责是把 GitHub issue、workspace、repo 结构、验证线索、执行风险和 agent 安全边界整理成 Codex、Cursor、Claude Code 等 agent 可以高质量消费的 handoff。

第一版采用“中等探测”边界：

- 允许 Patchbay 运行固定白名单的低风险探测命令。
- 不允许运行目标仓库检测出的 `test`、`lint`、`build`、安装依赖、网络型命令或用户脚本。
- 探测结果只进入 handoff 和报告，不触发自动代码修改。

## 产品边界

Patchbay 负责：

- 发现、排序和评估开源 issue。
- 准备本地 workspace 和 Patchbay 分支。
- 静态扫描 repo 结构、候选文件、贡献说明和验证线索。
- 在固定白名单内运行低风险探测命令。
- 生成 `handoff.json`、`handoff.md`、progressive context pack。
- 新增生成 `agent-policy.json`、`probe.json`、`context/probe.md` 和 `prepare-events.jsonl`。
- 明确告诉下游 agent 哪些动作可直接尝试，哪些需要用户批准，哪些禁止。

Patchbay 不负责：

- 启动或驱动 Codex、Cursor、Claude Code 等 agent。
- 实现模型 tool loop、approval loop 或通用 MCP host。
- 自动应用 patch、编辑目标源码、安装依赖、运行完整测试、提交、推送或创建 PR。
- 替代 Codex 的平台沙盒、权限审批和工具执行 runtime。

这个边界保持 Patchbay 的产品定位：它是高质量 agent 前置层，不是 agent 本体。

## Codex 参考

本设计参考 `reference/openai-codex/codex-rs` 的具体实现，但只借鉴适合 Patchbay 的思想和轻量结构。

### Sandbox 和权限模型

Codex 的 `codex-rs/protocol/src/protocol.rs` 定义了 legacy `SandboxPolicy`：

- `read-only`
- `workspace-write`
- `external-sandbox`
- `danger-full-access`
- network restricted/enabled

`codex-rs/protocol/src/models.rs` 定义了更现代的 `PermissionProfile`：

- `Managed`
- `External`
- `Disabled`
- `AdditionalPermissionProfile`
- `SandboxEnforcement`

`codex-rs/protocol/src/permissions.rs` 定义了文件系统和网络权限：

- `FileSystemSandboxPolicy`
- `FileSystemSandboxEntry`
- `FileSystemAccessMode::{Read, Write, Deny}`
- `NetworkSandboxPolicy`
- 对 `.git`、`.agents`、`.codex` 等 workspace metadata 的保护。

Patchbay 不复制 Codex 的平台沙盒执行器，但应借鉴这些语义，生成 agent 可读的 `agent-policy.json`。

### 平台沙盒执行器

Codex 的 `codex-rs/sandboxing/src/manager.rs` 负责把权限模型转换为平台执行方式：

- macOS Seatbelt
- Linux sandbox
- Windows restricted token
- sandbox selection
- network policy integration

Patchbay 第一版不移植这套 runtime。原因是 Patchbay 自己只运行固定白名单探测命令，不接受模型任意命令，因此不需要完整 approval -> sandbox -> retry/escalate 执行循环。

### 安全命令分类

Codex 的 `codex-rs/shell-command/src/command_safety/is_safe_command.rs` 有 `is_known_safe_command`，用于判断哪些命令 read-only enough to auto-approve，例如：

- `cat`
- `ls`
- `rg`
- `sed -n`
- `git status`
- `git log`
- `git diff`
- `git show`
- 只读形态的 `git branch`

它也会拒绝可能写文件或改变上下文的 flag，例如：

- `find -exec`
- `find -delete`
- `rg --pre`
- `git --git-dir`
- `git --work-tree`
- `git --exec-path`
- `git diff --output`

Codex 的 `is_dangerous_command.rs` 和 Windows 相关实现还覆盖了危险命令检测，例如 `rm -rf` 和 Windows shell 风险。

Patchbay 应借鉴这个思想，但不需要开放通用命令判断给用户或模型。Patchbay 的 `SafeProbeRunner` 使用固定 command enum 和参数构造，禁止 shell 拼接。

### ExecPolicy 和审批

Codex 的 `codex-rs/execpolicy` 定义了：

- `allow`
- `prompt`
- `forbidden`
- prefix rule
- network rule

Codex 的 `codex-rs/core/src/tools/orchestrator.rs` 把工具执行组织为：

```text
approval
  -> sandbox selection
  -> first attempt
  -> sandbox denial handling
  -> optional retry or rejection
```

Patchbay 不实现这个审批 runtime。Patchbay 应把同类语义固化进 `agent-policy.json`：

- `allow`
- `requires_user_approval`
- `forbidden`

真正的审批、沙盒和执行仍由 Codex 这类 agent runtime 负责。

### 事件和审计

Codex 的 `codex-rs/protocol/src/protocol.rs` 使用 Submission Queue / Event Queue 模式，并定义了大量事件：

- `TurnStarted`
- `TurnComplete`
- `ExecCommandBegin`
- `ExecCommandEnd`
- `ExecApprovalRequest`
- `RequestPermissions`
- `PatchApplyBegin`
- `PatchApplyEnd`

Patchbay 不需要完整会话协议，但可以借鉴事件化思路。第一版新增 `prepare-events.jsonl`，记录 prepare 过程中的关键事件，用于失败恢复、报告和 handoff 审计。

### 渐进式上下文披露

Codex 的 skill 和 dynamic tool 实现强调上下文预算：

- `codex-rs/core-skills/src/model.rs`：skill metadata、policy、scope、dependencies。
- `codex-rs/core-skills/src/render.rs`：skill 描述受预算限制，必要时截断或省略。
- `codex-rs/protocol/src/dynamic_tools.rs`：`defer_loading` 表示延迟加载。
- `codex-rs/core/src/mcp_tool_exposure.rs`：工具过多时转为 deferred exposure。

Patchbay 已有 progressive handoff pack。Agent-Safe Preparation Runtime 应继续沿用这个方向，把 probe 和 policy 作为延迟上下文，而不是把所有细节塞进 `codex.md`。

## 目标

1. 让 Patchbay 的 prepare 输出更适合 Codex 等 coding agent 接手。
2. 用纯算法和固定探测命令提高 repo 执行现场的可见性。
3. 用结构化 policy 表达安全边界，而不是只靠 Markdown 提醒。
4. 把探测结果、风险、建议命令和审批要求写入 canonical artifacts。
5. 保持 Patchbay 本身不依赖 LLM、不执行代码修改、不进入通用 agent runtime。

## 非目标

- 不实现 `patchbay agent`。
- 不接入 LLM provider 作为核心路径。
- 不生成或应用 patch。
- 不自动运行 repo 的测试、lint、build 或安装命令。
- 不移植 Codex 的完整 sandbox manager、tool orchestrator、approval cache 或 guardian review。
- 不把 `agent-policy.json` 当作强制执行的操作系统 sandbox。它是 handoff manifest 和 agent 协议，不是平台隔离机制。

## 输出结构

每个 prepared inbox item 生成：

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
    value.md
    issue.md
    repo.md
    validation.md
    probe.md
  .agents/
    skills/
      patchbay-cli/
        SKILL.md
        refs.json
```

`handoff.json` 继续作为 canonical 机器可读输出。新增字段应保持兼容，优先增加独立对象而不是改变既有字段含义。

## Agent Policy Manifest

`agent-policy.json` 是给下游 agent 的结构化安全协议。它不强制执行，但要尽量贴近 Codex 的权限语义。

建议结构：

```json
{
  "version": 1,
  "kind": "patchbay_agent_policy",
  "handoff_id": "2026-06-04-owner__repo-123",
  "permission_profile": {
    "filesystem": {
      "read_roots": [
        "/abs/path/to/workspace",
        "/abs/path/to/inbox/2026-06-04-owner__repo-123"
      ],
      "write_roots": [
        "/abs/path/to/workspace"
      ],
      "protected_roots": [
        "/abs/path/to/workspace/.git",
        "/abs/path/to/workspace/.agents",
        "/abs/path/to/inbox/2026-06-04-owner__repo-123"
      ]
    },
    "network": "requires_user_approval"
  },
  "commands": {
    "allowed_low_risk": [
      {
        "argv": ["git", "status", "--porcelain"],
        "cwd": "/abs/path/to/workspace",
        "reason": "Read workspace dirty state."
      }
    ],
    "requires_user_approval": [
      {
        "command": "pytest",
        "reason": "Detected validation command may execute repository code."
      }
    ],
    "forbidden": [
      {
        "pattern": "install dependencies",
        "reason": "Patchbay does not install dependencies or ask agents to install without user approval."
      }
    ]
  },
  "agent_constraints": [
    "Do not modify Patchbay inbox files.",
    "Do not modify .git, .agents, or generated context files.",
    "Ask the user before running commands that require network, dependency installation, tests, build, or lint."
  ]
}
```

Policy categories:

- `allowed_low_risk`: safe for Patchbay probes and safe for an agent to consider without extra explanation.
- `requires_user_approval`: useful but may run code, consume time, touch network, or require dependencies.
- `forbidden`: outside Patchbay's safety boundary or destructive.

## Safe Probe Runner

`SafeProbeRunner` runs only fixed probe definitions. It should not accept arbitrary strings from issue body, config, handoff, LLM output, or user-provided command text.

Probe command requirements:

- Represent commands as enum variants or strongly typed structs.
- Build `argv` arrays directly.
- Never run through `sh -c`, `bash -lc`, or string concatenation.
- Set cwd explicitly to the prepared workspace.
- Apply a short timeout.
- Capture stdout/stderr with byte and line limits.
- Record exit code, duration, truncation, and warnings.
- Treat failure as a probe warning, not as prepare failure, unless the probe is required for safety.

Initial probe set:

```text
GitStatusPorcelain        -> git status --porcelain
GitBranchShowCurrent      -> git branch --show-current
GitLsFiles                -> git ls-files
GitRemoteGetUrlOrigin     -> git remote get-url origin
CargoMetadataNoDeps       -> cargo metadata --no-deps --format-version 1
NpmPkgGetScripts          -> npm pkg get scripts --json
PnpmPkgGetScripts         -> pnpm pkg get scripts --json
PythonProjectMetadata     -> static parse pyproject.toml/setup.cfg where possible
```

Important boundary: `cargo metadata --no-deps` can execute some build-related metadata paths in unusual projects and may touch the network if dependency resolution is needed. First version should prefer static `Cargo.toml` parsing and only run `cargo metadata --no-deps` when the command is available, bounded, and marked as `probe_risk = "medium"`. If this is too risky in implementation, it should be omitted from first release.

Patchbay must not run:

- `cargo test`
- `cargo check`
- `npm install`
- `npm test`
- `pnpm install`
- `pytest`
- `make`
- project-defined scripts
- commands inferred directly from issue text

Those commands may be listed as validation candidates with `requires_user_approval`.

## Probe Pack

`probe.json` stores structured probe results:

```json
{
  "version": 1,
  "kind": "patchbay_probe_pack",
  "status": "completed",
  "started_at": "2026-06-04T00:00:00Z",
  "completed_at": "2026-06-04T00:00:03Z",
  "workspace": "/abs/path/to/workspace",
  "probes": [
    {
      "id": "git_status_porcelain",
      "argv": ["git", "status", "--porcelain"],
      "exit_code": 0,
      "duration_ms": 28,
      "stdout_excerpt": "",
      "stderr_excerpt": "",
      "risk": "low",
      "warnings": []
    }
  ],
  "facts": {
    "workspace_dirty": false,
    "current_branch": "patchbay/123-example",
    "package_managers": ["cargo"],
    "detected_scripts": [],
    "agent_instruction_files": ["AGENTS.md"],
    "validation_candidates": [
      {
        "command": "cargo test",
        "source": "Cargo.toml and repo scan",
        "approval": "requires_user_approval"
      }
    ]
  },
  "warnings": []
}
```

`context/probe.md` summarizes this for humans and agents:

- What was probed.
- What was learned.
- Which commands were not run and why.
- Which validation commands require user approval.
- Any probe warnings.

`codex.md` should point to `context/probe.md` after `context/safety.md` and before code planning.

## Prepare Events

`prepare-events.jsonl` records one JSON object per event:

```json
{"type":"prepare_started","timestamp":"2026-06-04T00:00:00Z","issue":"owner/repo#123"}
{"type":"workspace_prepared","timestamp":"2026-06-04T00:00:01Z","path":"/abs/path/to/workspace","branch":"patchbay/123-example"}
{"type":"probe_started","timestamp":"2026-06-04T00:00:01Z","probe":"git_status_porcelain"}
{"type":"probe_completed","timestamp":"2026-06-04T00:00:01Z","probe":"git_status_porcelain","exit_code":0,"duration_ms":28}
{"type":"agent_policy_written","timestamp":"2026-06-04T00:00:02Z","path":"agent-policy.json"}
{"type":"handoff_written","timestamp":"2026-06-04T00:00:03Z","path":"handoff.json"}
```

Events should be append-only within a prepare run. If prepare fails, the event log should still capture the last completed step and failure reason.

## Execution Readiness Score

Patchbay currently scores issue value and execution suitability. Agent-Safe Preparation Runtime adds a preparation-focused readiness score that uses repo and probe facts.

Recommended axes:

- `workspace_state`: clean workspace, branch prepared, origin matches GitHub repo.
- `file_locality`: candidate files found and reasonably scoped.
- `validation_detectability`: validation candidates found without running project code.
- `setup_clarity`: contribution docs, package manifests, agent instructions found.
- `command_safety`: suggested commands classified into allowed/prompt/forbidden.
- `dependency_complexity`: signs of heavy setup, lockfile absence, multi-package complexity.
- `context_completeness`: issue, repo, value, validation and probe context all generated.

This score should not replace value scoring. It should answer a different question:

```text
How prepared is this task for a coding agent to start safely?
```

Daily selection can use it as a tie-breaker or gate, but first release should only surface it in handoff and reports to avoid unexpected ranking churn.

## Data Flow

```text
prepare owner/repo#123
  -> fetch issue
  -> enrich issue
  -> compute value assessment
  -> clone/fetch workspace
  -> create/reuse Patchbay branch
  -> static repo scan
  -> safe probe runner
  -> build probe pack
  -> build agent policy
  -> compute readiness score
  -> write handoff/context/policy/probe/events
  -> upsert inbox
```

`daily` uses the same prepare path for each selected issue. Single issue failures continue to be recorded without stopping the full daily run.

## Error Handling

Probe failures should be non-fatal by default:

- Missing binary: record probe warning.
- Timeout: record timeout warning and mark probe incomplete.
- Non-zero exit: record exit code and stderr excerpt.
- Output too large: truncate and record truncation.
- Invalid UTF-8: lossy decode and record warning.

Fatal prepare errors remain:

- Cannot parse issue reference.
- Cannot fetch required issue metadata.
- Cannot clone or fetch workspace.
- Cannot write required handoff artifacts.
- Cannot create local state directories.

If policy or probe pack generation fails after workspace preparation, Patchbay should either:

1. write a handoff with explicit warnings, or
2. mark the inbox item `prepare_failed` if the missing artifact would make the handoff unsafe.

The preferred first-release behavior is to fail closed when `agent-policy.json` cannot be written, because the policy is the main safety contract.

## Testing

Unit tests:

- Probe command builders produce exact argv arrays.
- Probe runner rejects any non-enum or shell-string command path.
- Timeout and output truncation are recorded.
- Policy manifest protects `.git`, `.agents`, `.codex`, and Patchbay inbox files.
- Validation commands are classified as `requires_user_approval`, not `allowed_low_risk`.
- Readiness score axes produce stable results for fixture repos.

Integration tests:

- `prepare` writes `agent-policy.json`, `probe.json`, `context/probe.md`, and `prepare-events.jsonl`.
- Dirty workspace is reflected in probe facts and readiness score.
- Missing package manager binaries do not fail prepare.
- `daily` records probe warnings in the report.
- Existing progressive context pack includes the new probe context entry.

Regression tests:

- Patchbay never runs project-defined scripts during prepare.
- Patchbay never runs install/test/lint/build commands during prepare.
- Patchbay does not write inside the target repo except existing allowed branch checkout behavior.
- Generated policy marks Patchbay inbox and generated context files as protected.

## Rollout

Phase 1:

- Add `agent-policy.json`.
- Add `prepare-events.jsonl`.
- Add `probe.json` with static-only facts and git probes.
- Add `context/probe.md`.

Phase 2:

- Add package-manager script discovery probes.
- Add readiness score.
- Surface readiness in `handoff.md`, `handoff.json`, `inbox`, and daily reports.

Phase 3:

- Add optional medium-risk metadata probes where justified.
- Add more Codex-compatible policy hints.
- Consider adapter-specific entries for Cursor and Claude Code.

No phase should introduce automatic patch generation, dependency installation, full validation execution, commit, push, or PR creation without a separate design review.

## Open Design Choices Resolved

- Patchbay will use “中等探测”: fixed low-risk probes, not full validation execution.
- Patchbay will not directly call LLMs as part of this runtime.
- Codex remains the downstream agent runtime.
- `agent-policy.json` is a handoff contract, not an OS-level sandbox.
- First implementation should prefer conservative omission over risky probe expansion.
