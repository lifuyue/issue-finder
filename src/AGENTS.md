# 源码目录规范

本文件适用于 `src/` 下的 Rust 源码，并在仓库根目录 `AGENTS.md` 的基础上补充源码目录的约定。

## 模块职责

保持现有模块边界清晰：`cli.rs` 负责命令行参数，`main.rs` 只做 CLI adapter 调度，`workflow.rs` 负责工作流编排、issue 选择和 options-based prepare 入口，`github.rs` 和 `github_enrichment.rs` 负责 GitHub 访问与补充信息，`value_scoring.rs` 和 `value_signals.rs` 负责高价值 issue 的本地算法，`prepare_gate.rs` 负责 prepare gate 的唯一策略实现，`workspace.rs` 负责本地 workspace 准备，`paths.rs`、`inbox.rs`、`report.rs` 和 `handoff.rs` 负责本地状态与输出。tool contract 相关逻辑保持拆分：`tool_runtime.rs` 负责 registry、execute 和 public contract，`tool_outputs.rs` 负责稳定结构化输出 DTO，`tool_context.rs` 负责 `read_context` 的固定 section 映射和路径安全。新增逻辑优先放入已有职责匹配的模块，只有在职责明确扩大时再新增模块。

## 代码风格

使用 Rust 2021 和 `rustfmt` 风格。函数、变量、模块和测试名使用 `snake_case`，类型使用 `PascalCase`。优先写小而明确的函数，避免把跨模块策略塞进工具函数。注释保持克制，只解释不明显的业务约束、外部 API 行为或安全边界。

## 错误处理

面向 CLI 的路径优先返回带上下文的 `anyhow::Result`，库内明确的领域错误可以使用 `thiserror`。不要吞掉外部命令、文件系统、HTTP 或 JSON 解析错误；需要降级时应让调用方能看到原因，并保持已有工作流的失败隔离语义。tool runtime 的参数错误和系统错误必须保持 `success=false`；业务 gate 阻止仍是 `success=true`、`status=blocked_by_gate`。

## 外部服务与异步

GitHub 和 LLM 调用必须经过现有客户端或模块边界，不要在工作流中散落新的 ad hoc HTTP 调用。新增请求应设置清晰的 URL、header 和解析结构，并能被 `tests/` 下的本地 mock server 覆盖。异步代码使用已有 `tokio` 模式，避免引入新的 runtime 或阻塞长任务。

## 本地状态与 Workspace 边界

所有 Issue Finder 状态路径都应通过 `IssueFinderPaths` 生成，不要手写 `~/.issue-finder` 路径。保持 Issue Finder 的保守边界：可以准备本地 workspace 和写 handoff/report，但源码逻辑不应在目标仓库中安装依赖、提交、推送或创建 PR。读取 handoff 上下文时只允许 `tool_context.rs` 中固定 section，不接受任意 path，并保持 symlink/path traversal 防护。

## 高价值 Issue 算法

修改 `value_scoring.rs`、`value_signals.rs`、`scoring.rs` 或 `github_enrichment.rs` 时，要保持信号含义可解释、评分阈值稳定，并同步考虑 `tests/` 下的高价值 issue 场景覆盖。除非是明确的阈值变更，否则避免让排序依赖脆弱的精确分数。

## Recommendation Ranking 边界

修改 `recommendation/` 下的 ranking、feed、quality、freshness 或 feedback 逻辑时，应保持 deterministic，并同步维护 `tests/fixtures/recommendation_eval/` 的离线评测样本或说明无需更新的原因。不要在 `workflow.rs`、`tool_runtime.rs`、`daily` 路径或 GitHub adapter 中复制 gate、质量规则或排序权重。

fallback discovery 应与 feed ranking 分层：GitHub adapter 负责取候选和来源标记，ranking/quality 模块负责解释为什么候选可见、降权或隐藏。新增权重必须能通过 recommendation eval fixture 解释和回归。

## Tool Contract

`issue-finder tools list` 和 `issue-finder tools call` 是 JSON-only adapter。不要在这些命令的 stdout 添加人类提示、日志或表格；stdout 必须保持单个 JSON object，调试信息应走 stderr 或测试断言。未来 MCP/Codex dynamic tool adapter 应复用 `tool_runtime.rs`，不要重新实现 gate、assessment、prepare 或 context 读取逻辑。

## 验证命令

- `cargo fmt --all -- --check`：确认格式。
- `cargo clippy --all-targets -- -D warnings`：确认 lint。
- `cargo test`：运行完整测试。
- `cargo test --test <name>`：针对修改影响的集成测试做快速验证。
