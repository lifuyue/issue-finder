# Codex Agent Thread 实现差异与落地清单

更新日期：2026-07-06

本清单记录 Issue Finder 当前 agent daemon/thread/tool-loop 与本地 `reference/codex` 的差异，以及按 Codex 架构 loop 和代码风格推进实现时应完成的具体改动、完成程度和验收方式。范围覆盖 agent 线程管理、工具注入、上下文管理、A2A/线程协议和结果回传；不覆盖推荐算法本身。

## 当前判断

- [x] Issue Finder 原先 agent loop 不是 Codex 等价实现；它主要是 system prompt 工具说明、`/chat/completions`、LLM 返回 JSON 文本、host 解析后执行工具。
- [x] Codex 的主路径是结构化 `ResponseItem`、Responses API tool specs、`call_id`/`turn_id` 生命周期、tool registry/router 和 app-server thread/turn 协议。
- [x] Issue Finder 已有可复用基础：`IssueFinderToolSpec`、`IssueFinderToolRuntime`、agent SQLite thread tables、dispatch 侧 Codex app-server adapter、session transcript artifact spill。
- [x] 2026-07-05 已落地 P0 provider/tool registry/call_id 改造：Responses native tool 是主路径，chat completions 仅作为 adapter 内 fallback。
- [x] 2026-07-05 已落地 thread mailbox、steer/interrupt/inject endpoint、确定性 context compaction、agent artifact spill、approval request flow 和 resumable event polling。
- [x] 2026-07-06 已补齐最后的可重复验收缺口：生产并行只开放经证明的只读工具，fake Responses LLM server 端到端驱动 thread tool loop，并检查 SQLite call id、result artifact 和 compaction item。

## 目标架构 Loop

目标不是复制 Codex 全量运行时，而是在 Issue Finder 边界内实现同类 loop：

```text
client or Codex
  -> Issue Finder A2A/thread endpoint
  -> AgentThreadStore writes thread/turn/input item
  -> AgentRuntime builds TurnContext
  -> AgentToolRouter exposes direct/deferred tools
  -> AgentLlmClient sends native Responses tool specs when supported
  -> model returns assistant message and/or tool calls
  -> AgentToolRouter executes IssueFinderToolRuntime
  -> store writes tool started/completed/result items and events
  -> AgentRuntime feeds function_call_output back to model
  -> loop repeats until final answer, cancellation, max turns, or needs approval
  -> A2A returns thread id, turn id, event stream URL, result/artifact refs
```

核心要求：

- [x] 模型可见工具来自结构化 schema，不再依赖 system prompt 中的 JSON 示例。
- [x] 所有工具执行都带稳定 `thread_id`、`turn_id`、`call_id`。
- [x] thread 可以继续，不论是新 turn、运行中 steer，还是只注入上下文 item。
- [x] 上下文构建有预算、摘要和 artifact spill，不是简单截断最近 N 条。
- [x] 对 Codex 返回的是可继续协调的 thread/turn/result/event 引用，而不是只返回最终自然语言。

## Codex 参考结论：算法边界

本轮对照 `reference/codex` 后的判断：不应把推荐 ranking/value scoring 整体改成 LLM 主导，也不应继续把 `scout` 这种批处理 pipeline 作为 agent 默认工具。Codex 的模式是稳定 loop + bounded tool observations + tool registry/exposure + context manager；模型在每次 observation 后决定下一步，而不是等待一个超大工具一次性产出完整世界状态。

对应到 Issue Finder，长期方向是：

- [x] 保留现有 discovery、value scoring、feed ranking、prepare gate 的 owner 模块职责。
- [x] 把 agent 默认工具切成轻量、可组合、可审计的步骤：召回候选、查看单候选、深评明确 issue。
- [x] 补 `inspect_discussion`、`rank_shortlist`、`inspect_repo_health` 等中粒度工具，让模型按需拉取评论、时间线、维护者信号和短名单比较依据。
- [x] 对大 observation 做摘要和 artifact spill，避免上下文注意力被完整 scout 输出拉偏。

## P0：Provider 与 Native Tool Call

### P0.0 Agent-native Recommendation Tools

真实环境验证显示，`issue-finder.scout --limit 1` 仍会触发全局 discovery、fallback、enrichment、competition evidence completion 和 feed ranking 等批处理路径，容易让 agent turn 长时间停在单个 tool call，并把过大的 observation 塞回模型。它应保留为 CLI/daily/batch macro tool，但不应作为 agent daemon 的默认 direct 工具。

- [x] 新增 `issue-finder.discover_candidates`，用于轻量召回候选，只返回候选 id、issue 基本信息、source lanes、trust tier、matched labels、rough score 和短 body preview。
- [x] 新增 `issue-finder.inspect_candidate`，用于按 `owner/repo#number` 或 URL 查看单个候选 issue 的核心详情，不做 enrichment/ranking。
- [x] `issue-finder.assess` 继续作为深评工具，对明确 issue 复用现有 value scoring、enrichment、gate 输出。
- [x] agent daemon 默认 direct 工具改为 `status`、`discover_candidates`、`inspect_candidate`、`inspect_discussion`、`inspect_repo_health`、`rank_shortlist`、`assess`。
- [x] `issue-finder.scout` 保留在 public tool contract 中，但定位为 batch discovery/ranking pipeline，不再作为 agent direct 默认工具。

完成程度：

- [x] 轻召回不进入完整 `RecommendationEngine::scout`。
- [x] 轻召回默认只查少量 trusted lanes，输出有 `laneLimit`。
- [x] 单候选查看只抓 direct issue，不读取 comments/timeline/growth evidence。
- [x] 已补 `inspect_discussion`，把 comments/timeline/maintainer signal 作为独立可控步骤。
- [x] 已补 `rank_shortlist`，让模型在 1-5 个显式候选之间做最终比较。

验收：

- [x] 单测覆盖新工具 schema 和 agent card direct 工具清单。
- [x] mock GitHub runtime 测试覆盖 `discover_candidates -> inspect_candidate -> assess`。
- [x] 真实环境 smoke 证明 LLM 先调用轻工具，而不是直接进入 batch `scout`。

2026-07-05 smoke 记录：

- 使用隔离 `ISSUE_FINDER_HOME` 启动本地 daemon，agent card 暴露工具为 `status`、`discover_candidates`、`inspect_candidate`、`assess`，未暴露 `scout`。
- 新 thread `agent-thread-20260705063813461-1` 的首个 turn 真实调用顺序为 `discover_candidates -> inspect_candidate -> assess -> final`。
- 同一 thread 的 follow-up turn 没有重新全局 discover，继续执行 `inspect_candidate -> assess -> final`，证明 idle thread 可以追加下一轮工具调用并复用已有上下文。
- 后续实现已补运行中 steer/mailbox、context compaction、artifact spill 和 approval request flow；仍保留 native Responses 真实 smoke、背压和并行工具等后续项。

2026-07-05 provider/call-id smoke 记录：

- 使用隔离 `ISSUE_FINDER_HOME` 和真实 LLM/GitHub 配置启动本地 daemon；当前真实配置读取为 `wireApi=chat_completions`，因此验证的是 fallback 路径，agent card 明确标记 `tools.native_responses=unsupported`。
- 第一次真实 run 暴露 chat fallback provider 会连续输出两个 JSON object，旧 parser 会把首尾拼成非法 JSON；已修复为解析第一个有效 JSON object，并补单测。
- 同一次真实 run 暴露 raw provider response 中可能包含 `reasoning_content`；已在 provider adapter 层清洗隐藏推理字段，并补单测。修复后用 `rg reasoning_content` 检查临时 home 无落库命中。
- 修复后新 thread `agent-thread-20260705071252223-1` 首轮真实调用顺序为 `discover_candidates -> inspect_candidate -> assess -> final`。
- 同一 thread follow-up turn 真实调用顺序为 `inspect_candidate -> assess -> final`，证明 idle thread 续写和 SQLite thread history 对下一轮工具选择有效。

2026-07-05 thread-control/context smoke 记录：

- 真实 smoke 暴露模型会在 `discover -> inspect -> inspect` 后跳过 `assess` 直接 final；已新增 recommendation evidence gate，推荐类目标必须至少一次成功 `issue-finder.assess` 后才能 final。
- 真实 smoke 暴露 follow-up turn 在继续探索时可能耗尽工具轮次并失败；已新增 tool-budget-exhausted finalization 采样，预算耗尽后不再暴露工具，要求模型基于已观察证据收束。
- 隔离 `ISSUE_FINDER_HOME` 真实运行验证首轮 thread 调用 `discover_candidates -> inspect_candidate -> assess -> final`，running `thread-steer` 被 mailbox 记录并消费，同一 thread 的 follow-up turn 完成且没有因 max turns 失败。
- 当前真实配置仍为 `wireApi=chat_completions`，因此 native Responses 代码路径仍只由 fake transport 单测覆盖。

2026-07-05 thread-runtime/mid-tool smoke 记录：

- 隔离 `ISSUE_FINDER_HOME` 启动 daemon，agent card 暴露 direct tools：`status`、`discover_candidates`、`inspect_candidate`、`inspect_discussion`、`inspect_repo_health`、`rank_shortlist`、`assess`；capability matrix 暴露 `approval.approve`、`approval.reject`、`context.compaction`。
- 新 thread `agent-thread-20260705082540591-1` 首轮真实完成，SQLite lifecycle items 显示 runtime 执行 `discover_candidates -> inspect_candidate -> assess`，没有调用 batch `scout`。
- 同一 thread follow-up 真实完成，runtime 使用 `inspect_discussion -> rank_shortlist` 比较候选，证明新增中粒度工具能被 LLM 在后续 turn 中按需选择。
- 手动 `agent thread-compact` 创建 `context_compaction` item；SQLite 核对为 1 thread、2 turns、27 thread items、20 events、1 artifact spill，且 tool lifecycle items 保留 call/tool/result 轨迹。
- 临时 home 中用 `rg reasoning_content|chain-of-thought|思考链` 未发现隐藏推理字段落盘；验证后已删除临时 home。
- 当时真实配置仍为 `wireApi=chat_completions`，因此 native Responses smoke 尚未完成；后续见下一条 native Responses smoke 记录。

2026-07-05 native Responses smoke 记录：

- 使用真实配置副本在隔离 `ISSUE_FINDER_HOME=/tmp/issue-finder-agent-responses-smoke` 启动 daemon，并把副本配置改为 `wire_api = "responses"`；验证后删除临时 home。
- `agent card --json` 返回 `provider.wireApi=responses`、`provider.nativeTools=true`，并包含 compact `toolDefinitions`，每个 direct tool 都带 schema/exposure。
- 新 thread `agent-thread-20260705091910629-1` 首轮真实完成，native Responses output 为 `function_call` items，真实调用顺序包括 `status -> discover_candidates -> inspect_repo_health -> inspect_candidate x3 -> assess -> final`，未调用 batch `scout`。
- 同一 thread follow-up turn `agent-turn-20260705092033685-51` 完成，并基于已有上下文收束，没有重新跑全局 discovery。
- SQLite 核对为 2 个 completed turns、7 个 completed tool calls、7 个本地 result artifacts；所有 tool lifecycle events 均保留 `providerItemId`，completed events 均保留 `resultArtifactId`。
- 临时 home 中用 `rg reasoning_content|chain-of-thought|思考链` 未发现隐藏推理字段落盘。

2026-07-06 native Responses smoke 记录：

- 使用真实配置副本在隔离 `ISSUE_FINDER_HOME=/tmp/issue-finder-agent-smoke-20260706` 启动 daemon，并只在副本中插入 `wire_api = "responses"`；未修改真实 `~/.issue-finder/config.toml`。
- `agent card --json` 返回 `provider.wireApi=responses`、`provider.nativeTools=true`、`provider.deferredTools=true`，direct tools 为 `status`、`discover_candidates`、`inspect_candidate`、`inspect_discussion`、`inspect_repo_health`、`rank_shortlist`、`assess`，未暴露 batch `scout`。
- 新 thread `agent-thread-20260705161920949-1` 首轮真实完成，工具序列为 `status -> discover_candidates -> inspect_candidate -> assess -> final`，推荐候选为 `bytecodealliance/jco#402`。
- 同一 thread follow-up turn `agent-turn-20260705162037147-35` 真实完成，工具序列为 `inspect_discussion -> inspect_repo_health -> final`，证明后续 turn 复用同一 SQLite thread/context，而不是重新创建一次性 workflow。
- `agent thread-show --json` 核对为 2 个 completed turns、25 个 thread items、19 个 events、6 个 completed tool calls、6 个可读本地 result artifacts；所有 completed tool calls 均带 `providerItemId` 和 `resultArtifactId`。
- 同一真实 run 中出现 2 个 `context_compaction` item，分别保留 `strategy=deterministic_summary`、`coveredItemCount` 和 `retainedRecentItemCount`；大 observation 通过 artifact 引用追溯。
- 临时 home 中用 `rg reasoning_content|chain-of-thought|思考链` 未发现隐藏推理字段落盘。

### P0.1 Provider 抽象

- [x] 新增 `src/agent/llm_client.rs`。
- [x] 定义 `AgentLlmClient` trait，至少包含：
  - `sample_turn(request: AgentModelRequest) -> AgentModelResponse`
  - `wire_api() -> AgentWireApi`
  - `supports_native_tools() -> bool`
- [x] 新增 DTO：
  - `AgentModelRequest { model, input_items, tools, parallel_tool_calls, output_schema }`
  - `AgentModelResponse { items, usage, raw_provider_response }`
  - `AgentModelItem::{AssistantMessage, ToolCall, FunctionCallOutput, ReasoningSummary}`
  - `AgentToolCall { call_id, namespace, name, arguments, provider_item_id }`
- [x] `src/config.rs` 增加 `llm.wire_api`，取值至少支持 `responses` 和 `chat_completions`。
- [x] `responses` 是主路径；`chat_completions` 只是 fallback，并把旧 JSON decision 映射成同一套 `AgentModelItem`。

完成程度：

- [x] OpenAI-compatible Responses API 能收到 `tools` 字段和 input items。
- [x] Chat fallback 不再污染主循环类型；它只在 provider adapter 内做 JSON 文本兼容。
- [x] 配置缺失或 provider 不支持 native tools 时，错误信息明确指出当前 wire API 和缺失能力。

验收：

- [x] 单测：fake Responses transport 断言请求包含 `tools`，且没有把完整工具 JSON 示例塞进 system prompt。
- [x] 单测：fake Chat transport 返回旧 JSON decision 时，能映射成 `AgentToolCall`。
- [x] 单测：`wire_api = "responses"`、`wire_api = "chat_completions"` 都能解析。
- [x] 冒烟：`cargo test agent::` 中 agent llm client 模块测试通过。
- [x] 真实 `wire_api = "responses"` smoke 证明 agent card 暴露 `nativeTools=true`，provider 返回 `function_call` item id，runtime 执行 native tool calls 并回填结果。

### P0.2 Issue Finder Tool Spec 转 Responses Tool

- [x] 新增 `src/agent/tool_registry.rs`。
- [x] 定义 `AgentToolExposure::{Direct, Deferred, Hidden, ApprovalRequired}`。
- [x] 定义 agent tool definition/executor 等价结构，绑定：
  - canonical tool name
  - `IssueFinderToolSpec`
  - exposure
  - supports parallel flag
  - runtime execute
- [x] 从 `list_tool_specs()` 生成 agent registry，但执行仍委托 `IssueFinderToolRuntime`。
- [x] direct 初始工具：
  - `issue-finder.status`
  - `issue-finder.discover_candidates`
  - `issue-finder.inspect_candidate`
  - `issue-finder.assess`
- [x] deferred 初始工具：
  - `issue-finder.scout`
  - `issue-finder.prepare`
  - `issue-finder.read_context`
  - dispatch、GitHub、memory 管理类工具
- [x] approval-required 初始工具：
  - prepare 后续会写 workspace/handoff 的路径
  - dispatch approve/execute
  - GitHub post/comment 类工具

完成程度：

- [x] Agent loop 不能维护一份独立 allowlist 字符串；可见性必须来自 registry。
- [x] tool schema 单一来源仍是 owner module 的 `src/tool_specs.rs`。
- [x] `IssueFinderToolRuntime` 仍是唯一执行入口，agent 不复制 scout/assess/prepare 逻辑。

验收：

- [x] 单测：Responses tools JSON 中包含 `issue-finder.scout` schema。
- [x] 单测：deferred/approval tools 不进入首轮 model-visible tools。
- [x] 单测：未知 tool 或 deferred tool call 返回 model-facing error，不 panic。
- [x] `cargo test tools_list_outputs_stable_issue_finder_specs` 通过，agent registry 不破坏 JSON tool contract。

### P0.3 Tool Call / Tool Result 生命周期

- [x] `AgentToolCall` 必须保留 provider 返回的 `call_id`。
- [x] `AgentThreadItem` 持久化新增或等价表达：
  - [x] provider item id
  - [x] call id
  - [x] namespace/tool name
  - [x] arguments
  - [x] result status
  - [x] result payload summary
  - [x] result artifact id
- [x] 执行前写 `tool_call_started` item/event。
- [x] 执行后写 `tool_call_completed` 或 `tool_call_failed` item/event。
- [x] 回模型的 result item 使用同一个 `call_id`。

完成程度：

- [x] 不再用本地 item id 替代 provider tool call id。
- [x] 一次 turn 内多个 tool call 可以独立追踪。
- [x] 失败结果也必须回模型，除非 fatal provider/runtime 错误终止 turn。

验收：

- [x] 单测：模型返回 call id `call_123`，store 中 started/completed/result 都关联 `call_123`。
- [x] 单测：tool failure 作为 function_call_output 返回给模型，下一轮可继续。
- [x] 单测：相同 turn 中两个 tool call 顺序持久化稳定。
- [x] 单测：Responses parser 保留 provider item id，thread tool lifecycle item/event 保留 provider item id 且 completed/failed event 关联 result artifact id。

2026-07-05 provider/result artifact 验证记录：

- `cargo test responses_function_call_keeps_provider_call_id_and_namespace -- --nocapture`
- `cargo test thread_tool_call_lifecycle_keeps_provider_call_ids_and_order -- --nocapture`
- 真实 `wire_api=responses` smoke thread `agent-thread-20260705091910629-1` 中，SQLite 记录 7 次 tool call；started/completed events 均带 `providerItemId`，completed events 均带 `resultArtifactId`，本地 artifact 文件数为 7。

## P1：Thread Runtime 与 A2A 协议

### P1.1 AgentRuntime 分层

- [x] 将 `src/agent/llm_loop.rs` 拆成职责清晰的 owner 模块：
  - `llm_client.rs`：provider wire protocol
  - `tool_registry.rs`：tool specs、exposure、execution adapter
  - `context.rs`：thread context builder
  - `runtime.rs`：turn loop orchestration、approval resume adapter
  - `protocol.rs`：model request/input/output protocol helpers
- [x] `llm_loop.rs` 可保留为薄 re-export 或删除。
- [x] `AgentRuntime::run_turn` 只负责 loop：
  1. load turn/thread state
  2. build context
  3. sample model
  4. persist model items
  5. execute tool calls
  6. append tool outputs
  7. check mailbox/cancel/approval/final

完成程度：

- [x] 每个模块能单独读懂，不出现新的宽泛 `utils.rs`。
- [x] runtime 不直接构造 tool schema；只使用 registry。
- [x] server 不直接执行 tool；只创建 thread/turn/inject/cancel 请求并触发 runtime。

验收：

- [x] 单测覆盖 `AgentRuntime` 的一轮 tool call + final answer。
- [x] 单测覆盖 provider error、tool error、invalid tool call、max turn exhaustion。
- [x] `cargo clippy --all-targets -- -D warnings` 无 dead code/unused 警告。

2026-07-05 runtime fake provider 验证记录：

- 新增内部 `AgentLlmClient` 注入边界，生产路径仍由 `OpenAiCompatibleAgentLlmClient` 驱动，测试路径可注入 fake Responses provider。
- `agent::runtime` 单测覆盖完整 turn loop：模型返回 `issue-finder.status` tool call，runtime 通过 owner `IssueFinderToolRuntime` 执行并把 `FunctionCallOutput` 回填，模型第二轮 final 后 turn 完成。
- 同组单测覆盖 provider error 标记 turn failed、未知工具失败作为 tool output 回模型后仍可 final、非法 tool arguments 失败、tool budget 耗尽后 final request 禁用 tools 且无 final 时失败。

### P1.2 新建、继续、运行中注入

- [x] 保留现有 `/a2a/threads/start`，但返回更完整 envelope：
  - thread id
  - initial turn id
  - status
  - events URL
  - subscribe URL
  - result URL
- [x] 将 `/a2a/threads/{threadId}/turns/send` 语义改清楚：
  - thread idle/active 时创建新 turn
  - thread running 时不失败，转成 pending input 或 steer 请求
- [x] 新增或等价暴露：
  - `POST /a2a/threads/{threadId}/turns/send`
  - `POST /a2a/threads/{threadId}/inject-items`
  - `POST /a2a/threads/{threadId}/turns/{turnId}/steer`
  - `POST /a2a/threads/{threadId}/turns/{turnId}/interrupt`
- [x] 新增 mailbox 表或等价结构：
  - `agent_thread_mailbox_items`
  - `status = pending | consumed | rejected`
  - `delivery = new_turn | steer | inject_only | interrupt`

完成程度：

- [x] 用户或 Codex 可以在同一 Issue Finder thread 上继续发消息。
- [x] 如果 turn 正在跑，消息不会丢失，也不会因为 status=running 直接拒绝。
- [x] Runtime 在每次模型调用前和每次工具调用后检查 mailbox。

验收：

- [x] 单测：running thread 收到 follow-up 后，mailbox 记录 pending item。
- [x] 单测：runtime 消费 pending steer，并把它加入下一次 model input。
- [x] 单测：idle thread 收到 follow-up 会创建新 turn。
- [x] 单测：interrupt 后 turn 进入 cancelled，runtime 后续 loop 会停止。

### P1.3 A2A 返回给 Codex 的结果形状

- [x] `thread-start`、`turn-start`、`turn-detail` 返回统一 result refs：
  - `threadId`
  - `turnId`
  - `status`
  - `eventsUrl`
  - `subscribeUrl`
  - `itemsUrl`
  - `resultUrl`
  - `mailboxItem` when input was queued as steer
- [x] 结果不只返回 final answer；必须能让 Codex 继续读取 tool logs 和发送 follow-up。
- [x] 对兼容旧 `/a2a/tasks/send` 的一 shot task，内部也应创建 thread/turn 或明确标为 legacy。

完成程度：

- [x] A2A 不再只是自然语言任务入口，而是本地 thread control plane。
- [x] legacy task 明确标记 `legacy: true` 和 `threadRef: null`，不再伪装成 thread store 状态真相。

验收：

- [x] CLI `agent thread-start --wait --json` 返回 `thread`、`turn`、`eventsUrl`、`itemsUrl`、`resultUrl`。
- [x] CLI `agent thread-send --wait --json` 能继续同一 thread；running thread 会返回 `mailboxItem`。
- [x] 单测：旧 task endpoint 返回的 detail 能映射到 thread/turn refs，或明确 `legacy: true`。

### P1.4 Capability Matrix

- [x] agent card 增加 capability matrix：
  - `thread.start`
  - `thread.read`
  - `thread.list`
  - `turn.start`
  - `turn.steer`
  - `turn.interrupt`
  - `thread.inject_items`
  - `events.subscribe`
  - `tools.native_responses`
  - `tools.deferred`
  - `context.compaction`
- [x] 每个 capability 记录：
  - `supported | unsupported | experimental`
  - method/path
  - limits
  - provider requirements

完成程度：

- [x] Codex 或用户侧 agent 可以先读 card 决定该新建 thread、继续 thread、steer 还是轮询结果。
- [x] 不支持的能力要显式列出，不靠 404 表达。

验收：

- [x] 单测：agent card 包含 capability matrix 和当前 provider wire API。
- [x] CLI `agent card --json` 输出可被稳定断言。

2026-07-05 CLI card 验证记录：

- 新增 `agent_card_json_output_has_stable_thread_tool_shape`，断言 card JSON 的 `kind`、direct tools、capability path 和 provider capability 字段。
- 新增 `agent_card_json_command_parses_stably`，断言 `issue-finder agent card --json` CLI surface 不回退。

## P2：上下文、压缩、持久化

### P2.1 Context Builder

- [x] 新增 `src/agent/context.rs`。
- [x] 构造 model input 时必须包含：
  - [x] system/developer safety boundary
  - [x] thread goal
  - [x] compact summary if any
  - [x] recent user/assistant items
  - [x] latest user input or mailbox injected input
  - [x] unresolved tool calls/results 的专门恢复表达
  - [x] tool_search/deferred tool discovery result if relevant
- [x] 不再只取最近 12 条 item。
- [x] token 估算先可用字符近似，接口上保留替换成 tokenizer 的边界。

完成程度：

- [x] 上下文预算可配置，默认保守。
- [x] 大工具结果不会完整塞回后续每轮，只保留摘要和 artifact ref。
- [x] final answer 仍由 system instruction 约束为只基于 observed tool results；还需要继续扩大真实 smoke 覆盖。

验收：

- [x] 单测：超过预算的历史会生成 compact summary item。
- [x] 单测：最近用户输入和 mailbox steer 总是保留。
- [x] 单测：大 payload 被 summary + artifact ref 替代。
- [x] 单测：历史中未闭合的 tool call/result 会在下一轮 context 中形成独立 recovery message，且不会把当前 turn 的进行中 tool call 当成历史恢复状态。
- [x] 单测：新 turn context 包含 deferred/approval tool discovery 摘要，优先列出 `scout`、`prepare`、`read_context`，但不把这些工具暴露为当前 direct callable tools。

### P2.2 Compaction

- [x] 新增 compaction item type：
  - `context_compaction`
  - deterministic payload fields `strategy`、`coveredItemCount`、`retainedRecentItemCount`
- [x] compaction 可由三种方式触发：
  - 超过 context budget
  - 手动 endpoint/CLI
  - turn 完成后后台维护
- [x] 第一版使用 deterministic summary；未来可替换为 provider summary。

完成程度：

- [x] compaction 结果是 thread item，不覆盖原始历史。
- [x] 原始历史仍可通过 items/artifacts 追溯。
- [x] 摘要不能包含未观察到的事实。

验收：

- [x] 单测：compaction 后下一轮 context 包含 summary 和最新 turn。
- [x] 单测：compaction 不删除原始 items。
- [x] 单测：deterministic compaction 生效；provider compaction 尚未实现。

### P2.3 Artifact Spill

- [x] agent 增加本地 artifact 存储，路径位于 Issue Finder home 的 `agent/artifacts/`。
- [x] 大 payload、大 tool output、完整 provider raw response 超过阈值时写 artifact。
- [x] SQLite item 中保留：
  - storage = inline | artifact
  - artifact id
  - content type
  - sha256
  - summary
- [x] 可参考 dispatch transcript spill，但不要把 agent 状态强行塞进 dispatch store。

完成程度：

- [x] SQLite 不再无限膨胀。
- [x] 读 thread detail 时默认返回摘要；需要完整 payload 时按 artifact id 读取。

验收：

- [x] 单测：超过阈值的 payload 写 artifact，sha256 元数据入库。
- [x] 单测：thread detail 默认不内嵌大 payload。
- [x] 单测：artifact 缺失或 sha mismatch 返回明确错误。

## P3：事件流、审批、并行

### P3.1 Event Subscribe 与背压

- [x] 新增 subscribe endpoint 或 SSE：
  - `GET /a2a/threads/{threadId}/subscribe?since=<sequence>`
- [x] 事件必须包含 sequence，客户端断线后可从 sequence 恢复。
- [x] subscribe replay 有容量限制，超过上限返回 lagged。

完成程度：

- [x] GET events 轮询仍可用。
- [x] subscribe 第一版是 resumable polling；通过单次 replay 上限实现 lagged/backpressure，SSE 仍未实现。

验收：

- [x] 单测：subscribe 从指定 sequence 开始返回事件。
- [x] 单测：subscribe replay 超过容量时返回明确 lagged，不无限回放。

### P3.2 Approval-required Tool Flow

- [x] approval-required tool 被模型调用时，不直接执行危险动作。
- [x] runtime 把 pending approval result 返回模型。
- [x] runtime 创建可由用户批准并恢复执行的 agent approval request。
- [x] 用户批准后，后续 turn 可继续执行或重新调用。
- [x] prepare/dispatch/GitHub posting 仍复用 owner module policy。

完成程度：

- [x] agent 不绕过 `prepare_gate.rs`、dispatch approval、GitHub interaction policy。
- [x] 模型能看到“需要用户批准”的结构化结果，而不是静默失败。

验收：

- [x] 单测：模型调用 approval-required tool 会产生 pending approval，不执行动作。
- [x] 单测：批准后执行仍走 owner runtime。

### P3.3 Parallel Tool Calls

- [x] registry 层声明 `supports_parallel_tool_calls`。
- [x] 默认所有工具串行。
- [x] 只读、互不写状态的工具可开放并行；当前仅 `status` 和 `inspect_candidate` 标记为 parallel-safe。

完成程度：

- [x] 即使 provider 返回多个 tool calls，默认也按稳定顺序执行。
- [x] 允许并行前必须证明不会写共享状态或破坏曝光记录；`discover_candidates`、`inspect_discussion`、`inspect_repo_health`、`rank_shortlist`、`assess` 因 cache/read-recording 或较重 observation 仍保持串行。

验收：

- [x] 单测：两个 tool call 默认稳定顺序执行。
- [x] 单测：标记 parallel 的 fake tool 可并行执行。

2026-07-06 并行工具验证记录：

- `supportsParallel=true` 只出现在 `issue-finder.status` 和 `issue-finder.inspect_candidate`；tools contract 和 agent registry 单测均断言 `discover_candidates`、`inspect_repo_health`、`rank_shortlist`、`assess` 不支持并行。
- `cargo test parallel_thread_tool_calls_run_fake_executor_concurrently_and_commit_in_order -- --nocapture` 通过；fake executor 使用 barrier 证明两个 parallel-safe calls 同时执行，同时 SQLite started/completed lifecycle 仍按 callIndex 稳定提交。

## 代码风格要求

- [x] 按 owner 模块实现，不复制策略：
  - prepare gate 只在 `src/prepare_gate.rs`
  - tool schema 只在 `src/tool_specs.rs` 和 dispatch owner specs
  - tool execution 只通过 `src/tool_runtime.rs`
  - dispatch 行为仍归 `src/dispatch/*`
- [x] 模块保持小边界，避免新建宽泛 `utils.rs`。
- [x] DTO 使用 `serde(rename_all = "camelCase")`，枚举使用显式 string parse/as_str 模式，与现有代码一致。
- [x] CLI JSON 输出保持单个 JSON object，日志走 stderr/tracing。
- [x] 文档和 usage 同步更新，不能留下旧的“只支持 status/scout”误导说明。
- [x] 不提交真实 token、真实 `ISSUE_FINDER_HOME`、LLM 真实响应缓存或用户状态。

## 验收矩阵

### 自动测试

- [x] `cargo fmt --all -- --check`
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] agent provider fake transport tests
- [x] agent tool registry tests
- [x] agent runtime multi-turn tests
- [x] A2A/thread endpoint tests
- [x] context compaction/artifact spill tests
- [x] tools contract tests确认未破坏 `tools list` 和 `tools call`

2026-07-06 最终自动验证记录：

- `cargo fmt --all -- --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo run -- tools list`，确认 quick start 仍是 `discover_candidates`，agent direct tools 为 `status`、`discover_candidates`、`inspect_candidate`、`inspect_discussion`、`inspect_repo_health`、`rank_shortlist`、`assess`，parallel tools 仅为 `status`、`inspect_candidate`。

### 本地 mock 验收

- [x] 使用 fake LLM server 跑：
  - start thread
  - model calls `discover_candidates`
  - tool result returned
  - model calls `inspect_candidate`
  - tool result returned
  - model calls `assess`
  - final answer
  - follow-up continues same thread
- [x] 检查 SQLite：
  - thread/turn/items/events 持久化
  - call id 一致
  - result artifact 可读
  - compaction item 可追溯

2026-07-06 fake LLM server 验收记录：

- 新增 `fake_responses_llm_drives_thread_tools_and_persists_sqlite_history`，使用本地 fake Responses LLM server 和本地 mock GitHub API，不依赖真实 token、真实 LLM、真实 GitHub 或用户状态。
- 首轮 thread 由 fake Responses `function_call` 驱动真实 runtime 执行 `discover_candidates -> inspect_candidate -> assess -> final`；follow-up 在同一 thread 中继续执行 `inspect_candidate -> assess -> final`。
- 测试断言首个 LLM request 包含 native tool schema 且 `parallel_tool_calls=true`，后续 request 包含 `function_call_output(call_discover)`，证明 tool result 已回填给模型。
- SQLite 核对 2 个 completed turns、5 个 completed tool calls、稳定 provider item id、稳定 call id、可读 result artifact、final answer 和 `context_compaction` item。

### 真实环境冒烟

真实运行不作为 CI 必需，但完成实现后应使用隔离 home 验证一次：

```bash
ISSUE_FINDER_HOME=/tmp/issue-finder-agent-smoke cargo run -- agent daemon
ISSUE_FINDER_HOME=/tmp/issue-finder-agent-smoke cargo run -- agent thread-start "搜索全网仓库并推荐 issue" --limit 3 --wait --json
ISSUE_FINDER_HOME=/tmp/issue-finder-agent-smoke cargo run -- agent thread-send <thread-id> "进一步评估第一个候选" --wait --json
ISSUE_FINDER_HOME=/tmp/issue-finder-agent-smoke cargo run -- agent thread-show <thread-id> --json
```

通过标准：

- [x] 第一轮请求实际包含 native tool schema，或在 fallback 模式中明确记录 `wireApi=chat_completions`。
- [x] 至少一次真实 tool call 由 Issue Finder agent runtime 执行，不由外部 Codex 代跑 CLI。
- [x] follow-up 继续同一 thread。
- [x] thread detail 能看到 tool call log、tool result、final answer。
- [x] 如果触发大输出，单测证明 payload 写 artifact，SQLite 只留摘要和引用；真实环境已通过 `inspect_repo_health` 大 observation/result artifact 路径核对。

## 非目标

- [x] 不直接复制 Codex 的完整执行环境、沙箱、文件修改、PR 创建能力。
- [x] 不让 Issue Finder agent 绕过现有 prepare gate、dispatch approval、GitHub interaction policy。
- [x] 不在 agent loop 中重新实现推荐排序、prepare gate 或 dispatch policy；继续复用 owner 模块。
- [x] 不把所有工具一次性暴露给模型；高风险和大上下文工具必须 deferred 或 approval-gated。
- [x] 不记录或展示隐藏 chain-of-thought；只记录可审计的决策摘要、tool call、tool result、事件和 final answer。

## Definition of Done

- [x] 新建对话时，工具 schema 以 native tool 形式注入上下文；fallback 模式有明确标记。
- [x] LLM 能在同一 turn 内连续选择 Issue Finder 工具，host 执行并把结果回填给模型。
- [x] 同一 thread 支持后续消息、运行中 steer、interrupt 和事件订阅。
- [x] A2A/Codex 调用方拿到 thread/turn/event/result refs，可继续协调同一 Issue Finder thread。
- [x] 上下文管理有 budget、compaction、artifact spill，SQLite 线程控制真正参与恢复和续写。
- [x] 所有高风险工具仍受 owner policy 和 approval gate 约束。

仍未归入 Definition of Done 的后续工作：

- [x] 在真实 `wireApi=responses` provider 上重跑 native Responses smoke。
- [x] 补 provider item id/result artifact id 的完整入库链路。
- [x] 补受证明的并行只读工具执行。
