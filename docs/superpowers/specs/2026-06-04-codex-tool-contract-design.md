# Codex-Like Tool Contract 设计

日期：2026-06-04

状态：已实现

## 摘要

Issue Finder 下一阶段应从“只生成 handoff 文件给 coding agent 阅读”升级为“提供 Codex-like tool contract，让 agent 通过结构化 tool call 获取、筛选、准备和按需读取上下文”。handoff 文件继续存在，但它的产品定位变为持久化 evidence/context store，而不是 agent 交互的主协议。

本设计采用 C 方案：先定义稳定的内部 tool runtime contract，再用 CLI JSON adapter 落地第一版，后续可复用同一 runtime 接 MCP 或 Codex dynamic tool adapter。第一版不直接实现 MCP server，但所有输入输出都保持与 Codex tool-call 形态兼容。

## Codex 参考结论

参考实现主要位于 `reference/openai-codex/codex-rs`：

- `tools/src/tool_executor.rs`：`ToolExecutor` 将 tool name、tool spec、exposure、parallel support 和 handle runtime 绑定在一起。
- `tools/src/tool_call.rs`：tool runtime 收到的是 `call_id`、`turn_id`、tool name、payload、model 和 conversation history 等结构化调用上下文。
- `tools/src/tool_output.rs`：tool output 最终转成带 `call_id` 的 model-facing response item。
- `core/src/tools/spec_plan.rs`：按 turn context 组装 model-visible specs，同时保留 hidden/deferred runtime。
- `core/src/tools/router.rs`：把模型返回的 `ResponseItem::FunctionCall` 还原成内部 `ToolCall`。
- `core/src/tools/registry.rs`：集中执行 tool，处理 pre/post hooks、telemetry、生命周期事件和结果替换。
- `protocol/src/dynamic_tools.rs`：dynamic tool 的稳定协议是 `DynamicToolSpec`、`DynamicToolCallRequest`、`DynamicToolResponse`。
- `core/src/tools/handlers/tool_search.rs`：deferred tools 通过 `tool_search` 渐进式发现，而不是把所有工具和上下文一次性塞给模型。
- `hooks/src/events/pre_tool_use.rs` 与 `hooks/src/events/post_tool_use.rs`：hooks 的审计输入包含 `session_id`、`turn_id`、`tool_name`、`tool_input`、`tool_use_id`、`tool_response`。

核心借鉴：

- 主协议是结构化 schema、`call_id`、tool invocation 和 structured output，不是“让模型自己读 handoff 文件”。
- 大上下文应 deferred loading，按需读取。
- model-visible specs 与 runtime registry 应分离。
- gate/policy 应在 tool runtime 层执行，结果作为结构化 output 返回。

## 目标

- 提供 Issue Finder Tool Contract v1，让 coding agent 通过结构化 tool call 使用 Issue Finder。
- 保留现有 scout、assessment、prepare、handoff、context pack 能力，并把它们包装成稳定 tool runtime。
- 让 gate 成为 `prepare` 的前置策略，默认阻止低价值或低确定性目标进入准备流程。
- 将低深度填充、低信任仓库、竞争 PR 饱和等判断作为结构化结果暴露，而不是只写进 Markdown。
- 先落地 CLI JSON adapter，方便本地验证和后续 MCP/Codex adapter 复用。
- 保持 Issue Finder 安全边界：只准备本地 workspace 和 handoff，不修改目标仓库源码，不提交，不推送，不创建 PR。

## 非目标

- 不把 Issue Finder 改成自治 coding agent。
- 不在 v1 直接实现 MCP server。
- 不替换现有 `scout`、`prepare`、`daily` 人类 CLI 工作流。
- 不让 agent 绕过 gate 后静默 prepare。
- 不把 handoff 文件删除或废弃。
- 不基于 LLM 做核心 gate 判断。

## 产品原则

### Tool-Call First, Handoff Second

agent 首先调用 tool 获取结构化结果。handoff、context pack、probe pack、agent policy 等文件是 tool output 指向的持久化证据和上下文。

### Gate Before Prepare

`issue-finder.prepare` 默认只准备高价值目标：

- `high_value_ready`
- `high_value_needs_scoping`

其他分类默认阻止：

- `niche_but_actionable`
- `contested_or_low_trust`
- `needs_triage`
- `filtered_low_depth`

阻止不是系统错误，而是正常业务结果：`success=true`，`status=blocked_by_gate`。

### Explicit Bypass Only

`niche_but_actionable` 和其他被 gate 阻止的目标只有在调用方显式传入 `allowGateBypass=true` 且 `bypassReason` 非空时才能继续 prepare。bypass reason 必须进入 tool output，并写入 handoff/workspace warning 或 prepare event，避免 agent 静默绕过产品策略。

### Progressive Context

`scout`、`assess`、`prepare` 默认返回 compact summary 和 evidence refs。完整 issue body、repo context、validation context 和 handoff JSON 通过 `issue-finder.read_context` 按需读取。

### Adapter Is Thin

CLI JSON adapter、未来 MCP adapter、未来 Codex dynamic tool adapter 都不应重新实现业务逻辑。它们只负责：

- 解析 call envelope
- 调用 tool runtime
- 序列化 output envelope

## 架构

```text
existing workflows
  scout / assess_issue / prepare_value_issue / read_handoff
        |
tool_runtime
  ToolSpec
  ToolInvocation
  ToolOutput
  ToolRegistry
  gate policy
        |
adapters
  CLI JSON adapter v1
  MCP adapter later
  Codex dynamic tool adapter later
```

建议新增模块：

```text
src/tool_runtime.rs
src/tool_specs.rs
src/tool_outputs.rs
src/tool_context.rs
```

如果实现时更适合保持单文件，也可以先用 `src/tool_runtime.rs` 聚合 v1，再在后续拆分。

## Runtime Contract

### ToolSpec

Issue Finder tool catalog 应接近 Codex `DynamicToolSpec`，但 catalog metadata 与 runtime execution 应保持分离。`src/tool_specs.rs` 负责 tool specs、input schema builders、agent onboarding metadata 和 `list_tool_specs()`；`src/tool_runtime.rs` 只负责 invocation、dispatch、具体 tool call 和 runtime error mapping。

```rust
pub struct IssueFinderToolSpecsEnvelope {
    pub kind: String,
    pub version: u8,
    pub quick_start: ToolQuickStart,
    pub recommended_workflow: Vec<ToolWorkflowStep>,
    pub tools: Vec<IssueFinderToolSpec>,
}

pub struct IssueFinderToolSpec {
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub defer_loading: bool,
}
```

v1 namespace 固定为 `issue-finder`。模型或 adapter 看到的完整名为：

```text
issue-finder.status
issue-finder.scout
issue-finder.assess
issue-finder.prepare
issue-finder.read_context
```

### ToolInvocation

```rust
pub struct IssueFinderToolInvocation {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub arguments: serde_json::Value,
}
```

`call_id` 由调用方传入；CLI adapter 未传入时可生成稳定前缀，例如 `issue-finder-call-{timestamp}`。

### ToolOutput

```rust
pub struct IssueFinderToolOutput {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub success: bool,
    pub status: String,
    pub content_items: Vec<IssueFinderContentItem>,
    pub structured_content: serde_json::Value,
}
```

content item v1：

```rust
pub enum IssueFinderContentItem {
    InputText { text: String },
}
```

输出应同时满足两类消费者：

- 人类 CLI：能读 `content_items[0].text`
- agent/runtime：读 `structured_content`

系统错误使用 `success=false`，例如 JSON 参数非法、GitHub 请求失败、本地 IO 失败。

业务阻止使用 `success=true`，例如 `blocked_by_gate`。

## Tools

### `issue-finder.status`

用途：在 agent 调用 discovery、assessment 或 prepare 前，返回本地配置和 GitHub auth 可用性。

输入：

```json
{
  "checkAuth": true
}
```

输出 `structured_content` 不包含 token，只包含 config 路径存在性、token 来源、GitHub login 检查结果和下一步修复命令：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.status",
  "status": "ready",
  "success": true,
  "config": {
    "path": "~/.issue-finder/config.toml",
    "exists": true,
    "loadOk": true,
    "loadError": null
  },
  "github": {
    "tokenSource": "env:GITHUB_TOKEN",
    "auth": {
      "checked": true,
      "ok": true,
      "login": "octocat",
      "error": null
    }
  },
  "nextFixCommand": null
}
```

### `issue-finder.scout`

用途：发现并排序候选，返回 gate-aware compact list。

输入：

```json
{
  "limit": 10,
  "repo": null,
  "refresh": false,
  "includeFiltered": false,
  "recordExposure": true
}
```

字段：

- `limit`：返回候选数，默认 10。
- `repo`：可选仓库范围，格式为 `owner/name`；未传入时使用全局发现。
- `refresh`：是否忽略 GitHub discovery/enrichment cache。
- `includeFiltered`：是否包含 `filtered_low_depth`。
- `recordExposure`：是否记录候选曝光事件，默认记录。

`scout` 不提供 `minCategory` 这类分类下限参数。`RecommendationCategory` 是 gate 和解释分类，不是稳定线性等级；调用方应直接读取返回的 `category`、`gates` 和 `missingEvidence` 做决策。

输出 `structured_content`：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.scout",
  "status": "ok",
  "candidates": [
    {
      "issue": {
        "repoFullName": "owner/repo",
        "number": 123,
        "title": "Fix parser panic",
        "url": "https://github.com/owner/repo/issues/123"
      },
      "category": "high_value_ready",
      "rankScore": 82,
      "scores": {
        "repoInfluence": 90,
        "profileFit": 75,
        "executionQuality": 80,
        "risk": 5
      },
      "gates": {
        "lowDepth": { "status": "pass", "band": "strong", "reasons": [] },
        "repoInfluence": { "status": "pass", "band": "strong", "reasons": [] },
        "competition": { "status": "pass", "band": "acceptable", "reasons": [] },
        "profileFit": { "status": "pass", "band": "strong", "reasons": [] }
      },
      "riskTags": [],
      "missingEvidence": []
    }
  ],
  "filteredCount": 0
}
```

默认行为：

- 不返回 `filtered_low_depth`，除非 `includeFiltered=true`。
- 排序沿用现有 category-first 排序。
- 输出必须包含 gate summary，不能只返回分数。

### `issue-finder.assess`

用途：评估单个 issue 是否值得准备，不创建 workspace，不写 inbox，不写 handoff。

输入：

```json
{
  "issue": "owner/repo#123",
  "url": null,
  "refresh": false
}
```

约束：

- `issue` 与 `url` 二选一。
- 必须 fetch issue 并执行 enrichment + value assessment。
- 不调用 `prepare_workspace`。

输出 `structured_content`：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.assess",
  "status": "ok",
  "issue": {},
  "assessment": {
    "category": "high_value_needs_scoping",
    "rankScore": 78,
    "gates": {},
    "scores": {},
    "riskTags": [],
    "missingEvidence": [],
    "competition": {
      "openPrRefs": 0,
      "closedPrRefs": 1,
      "attemptComments": 0,
      "competitionPoints": 1,
      "competitionBand": "clear"
    }
  },
  "prepareGate": {
    "defaultAllowed": true,
    "allowedCategories": ["high_value_ready", "high_value_needs_scoping"],
    "requiresBypass": false,
    "reasons": []
  }
}
```

### `issue-finder.prepare`

用途：在 gate 控制下准备 workspace 和 handoff。

输入：

```json
{
  "issue": "owner/repo#123",
  "url": null,
  "refresh": false,
  "allowGateBypass": false,
  "bypassReason": null
}
```

默认 gate：

```text
allow:
  high_value_ready
  high_value_needs_scoping

block:
  niche_but_actionable
  contested_or_low_trust
  needs_triage
  filtered_low_depth
```

blocked output：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.prepare",
  "status": "blocked_by_gate",
  "success": true,
  "issue": {},
  "assessment": {},
  "prepareGate": {
    "defaultAllowed": false,
    "requiresBypass": true,
    "blockedCategory": "niche_but_actionable",
    "reasons": [
      "Repository influence is below high-value threshold"
    ],
    "bypassAvailable": true
  }
}
```

prepared output：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.prepare",
  "status": "prepared",
  "success": true,
  "issue": {},
  "assessment": {},
  "handoff": {
    "id": "2026-06-04-owner-repo-123",
    "dir": "/abs/path",
    "handoffJsonPath": "/abs/path/handoff.json",
    "handoffMarkdownPath": "/abs/path/handoff.md",
    "codexMarkdownPath": "/abs/path/codex.md",
    "agentPolicyPath": "/abs/path/agent-policy.json",
    "probeJsonPath": "/abs/path/probe.json",
    "prepareEventsPath": "/abs/path/prepare-events.jsonl"
  },
  "readiness": {
    "score": 72,
    "band": "medium"
  },
  "gateBypass": null
}
```

bypass output 额外包含：

```json
{
  "gateBypass": {
    "allowed": true,
    "reason": "User explicitly wants this niche issue despite lower repo influence",
    "originalBlockedCategory": "niche_but_actionable"
  }
}
```

实现要求：

- `allowGateBypass=true` 但 `bypassReason` 为空时返回 `success=false` 参数错误。
- bypass reason 写入 output。
- bypass reason 应写入 handoff warning 或 prepare event，避免静默绕过。
- `prepare` 仍不得提交、推送、创建 PR。

### `issue-finder.read_context`

用途：按需读取已准备 handoff 的指定上下文片段。

输入：

```json
{
  "handoffId": "2026-06-04-owner-repo-123",
  "section": "entry",
  "maxBytes": 12000
}
```

允许 section：

```text
entry
safety
probe
value
issue
repo
validation
handoff_json
agent_policy
probe_json
```

路径映射：

```text
entry         -> context/entry.md
safety        -> context/safety.md
probe         -> context/probe.md
value         -> context/value.md
issue         -> context/issue.md
repo          -> context/repo.md
validation    -> context/validation.md
handoff_json  -> handoff.json
agent_policy  -> agent-policy.json
probe_json    -> probe.json
```

安全要求：

- 不接受任意 path。
- 不允许 `../`、绝对路径、symlink 跳出 handoff dir。
- `maxBytes` 默认 12000，上限 50000。
- 超限时截断并返回 `truncated=true`。

输出：

```json
{
  "kind": "issue_finder_tool_output",
  "tool": "issue-finder.read_context",
  "status": "ok",
  "handoffId": "2026-06-04-owner-repo-123",
  "section": "entry",
  "path": "/abs/path/context/entry.md",
  "truncated": false,
  "content": "..."
}
```

## CLI JSON Adapter

新增命令：

```bash
issue-finder tools list
issue-finder tools call issue-finder.status --arguments '{}'
issue-finder tools call issue-finder.scout --arguments '{"limit":10}'
issue-finder tools call issue-finder.assess --arguments '{"issue":"owner/repo#123"}'
issue-finder tools call issue-finder.prepare --arguments '{"issue":"owner/repo#123"}'
issue-finder tools call issue-finder.read_context --arguments '{"handoffId":"...","section":"entry"}'
```

可选 envelope 字段：

```bash
issue-finder tools call issue-finder.scout \
  --call-id call_123 \
  --turn-id turn_456 \
  --arguments '{"limit":10}'
```

`tools list` 输出：

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
  "tools": [
    {
      "namespace": "issue-finder",
      "name": "status",
      "description": "...",
      "inputSchema": {},
      "deferLoading": false
    }
  ]
}
```

`tools call` 输出完整 `IssueFinderToolOutput` JSON。CLI adapter 不输出人类表格，避免破坏 agent 消费。

## Gate Policy

新增内部 helper：

```rust
pub enum PrepareGateDecision {
    Allowed,
    Blocked {
        category: RecommendationCategory,
        reasons: Vec<String>,
        bypass_available: bool,
    },
    Bypassed {
        category: RecommendationCategory,
        reason: String,
    },
}
```

默认允许：

```rust
matches!(
    category,
    RecommendationCategory::HighValueReady
        | RecommendationCategory::HighValueNeedsScoping
)
```

阻止原因应优先使用 gate verdict 和 risk tags：

- low-depth hard fail
- repo influence hard fail or soft fail
- competition contested/saturated
- profile fit hard fail
- execution score too low
- missing evidence

## 与现有 Handoff 的关系

现有 handoff 输出保留：

- `handoff.json`
- `handoff.md`
- `codex.md`
- `context/*.md`
- `agent-policy.json`
- `probe.json`
- `prepare-events.jsonl`

变更点：

- tool output 必须返回这些路径，不要求 agent 从 Markdown 中发现路径。
- `context_pack.files[].defer_loading` 与 `issue-finder.read_context` 的 section 概念保持一致。
- bypass reason 应进入 handoff 或 prepare event。

## 错误语义

### `success=true`

业务上完成了 tool call：

- scout 成功
- assess 成功
- prepare 成功
- prepare 被 gate 阻止
- read_context 成功但内容被截断

### `success=false`

tool call 无法完成：

- 参数不是 JSON object
- `issue` 与 `url` 同时提供或都不提供
- `allowGateBypass=true` 但 `bypassReason` 为空
- GitHub 请求失败
- 本地 state 读取失败
- handoff id 不存在
- read_context section 非法

## 实现计划

### 阶段 1：Runtime Contract

- 新增 tool spec、invocation、output 数据结构。
- 实现 `list_tool_specs()`。
- 实现统一 JSON schema 构造 helper。
- 添加 envelope 序列化测试。

### 阶段 2：Scout/Assess Tools

- `issue-finder.scout` 调用现有 `workflow::scout`。
- 新增可复用单 issue assessment workflow，供 `assess` 和 `prepare` 共用。
- 输出 compact gate-aware candidate shape。

### 阶段 3：Prepare Gate

- 在 tool runtime 层实现 prepare gate。
- 默认阻止非 high-value prepare。
- 实现 explicit bypass，要求 reason。
- prepared 输出包含 handoff paths、readiness、probe status。

### 阶段 4：Read Context

- 基于 inbox index 查找 handoff。
- 只允许固定 section。
- 实现 max bytes 截断。
- 覆盖非法 section 和路径穿越。

### 阶段 5：CLI Adapter

- 新增 `issue-finder tools list`。
- 新增 `issue-finder tools call <tool> --arguments <json>`。
- 支持 `--call-id` 和 `--turn-id`。
- 保证 stdout 是单个 JSON object。

## 测试计划

新增集成测试：

- `tools_list_outputs_stable_issue_finder_specs`
- `tool_scout_returns_gate_aware_candidates`
- `tool_assess_does_not_write_handoff_or_inbox`
- `tool_prepare_blocks_niche_without_bypass`
- `tool_prepare_requires_bypass_reason`
- `tool_prepare_bypass_writes_reason`
- `tool_prepare_returns_handoff_paths`
- `tool_read_context_reads_allowed_section`
- `tool_read_context_rejects_unknown_section`
- `tool_read_context_truncates_large_content`

保留现有测试：

- `cargo test`
- ranking/gate tests
- handoff/context pack tests
- workspace/daily tests

## 验收标准

- `cargo test` 通过。
- `cargo clippy --all-targets -- -D warnings` 通过。
- `issue-finder tools list` 输出五个 tool specs。
- `issue-finder tools call issue-finder.status --arguments '{}'` 返回 config、token source 和 GitHub auth 诊断，不输出 token。
- `issue-finder tools call issue-finder.scout --arguments '{"limit":5}'` 返回 category-first 候选。
- `issue-finder tools call issue-finder.assess --arguments '{"issue":"owner/repo#123"}'` 返回完整 gate summary 且不写 handoff。
- `issue-finder tools call issue-finder.prepare --arguments '{"issue":"owner/repo#123"}'` 对非 high-value 默认返回 `blocked_by_gate`。
- `allowGateBypass=true` 且 `bypassReason` 非空时可以 prepare，并在 output 中可见 bypass reason。
- `read_context` 只能读固定 section。

## 后续扩展

### MCP Adapter

当 v1 contract 稳定后，可将 `IssueFinderToolSpec` 转成 MCP tool schema，并将 `IssueFinderToolOutput` 转成 MCP `CallToolResult`。

### Codex Dynamic Tool Adapter

可将 tool specs 映射为 Codex `DynamicToolSpec`：

```json
{
  "namespace": "issue-finder",
  "name": "scout",
  "description": "...",
  "inputSchema": {},
  "deferLoading": false
}
```

调用时将 Codex `DynamicToolCallRequest` 转成 `IssueFinderToolInvocation`，再将 output 转成 `DynamicToolResponse.contentItems`。

### Deferred Tool Search

如果未来 tool 数量变多，可参考 Codex `tool_search`：

- 默认只暴露 `issue-finder.status`、`issue-finder.scout`、`issue-finder.assess`、`issue-finder.prepare`
- 将 `read_context` 或更细粒度 evidence readers 标记为 deferred
- agent 需要时再发现和加载

## 已确认决策

- 采用 C 方案：稳定 runtime contract 优先，adapter 可替换。
- v1 先做 CLI JSON adapter，不直接做 MCP server。
- `prepare` 默认只允许 `high_value_ready` 和 `high_value_needs_scoping`。
- `niche_but_actionable` 必须显式 bypass 才能 prepare。
- bypass 必须提供非空 reason。
- handoff 文件保留，但不再作为主交互协议。
