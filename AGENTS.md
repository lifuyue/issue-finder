# 仓库指南

## 当前仓库简介

Issue Finder 面向 Codex 提供 GitHub issue 发现与评估，CLI 使用 Rust 2021 实现。Codex 负责选择、工作区准备、复现、修复、验证、审查及 PR 交付。

CLI skill 的 session 工具接口仅提供 `scout` 和 `assess`（contract version 2），配置及认证错误直接由业务调用返回。Cloud 环境负责安装、依赖、PATH 和支持的认证配置。旧 control/dispatch、handoff 和贡献记忆暂为兼容保留，不属于 Codex 默认工具契约；清理共享模块前必须检查这些调用方。

## 项目简要导览

- `skills/issue-finder-cli/`：CLI 流程指引、安装说明、工具参考和展示元数据。
- `src/main.rs`、`src/lib.rs`、`src/cli.rs`：CLI 入口、库导出和命令行参数定义。
- `src/workflow.rs`、`src/prepare_gate.rs`：工作流编排、issue 选择和 prepare gate 策略。
- `src/recommendation/`、`src/value_*.rs`、`src/scoring.rs`、`src/competition.rs`：推荐排序、价值评估和竞争分析。
- `src/github.rs`、`src/github_enrichment.rs`、`src/discovery.rs`：GitHub 访问、证据补充和候选发现。
- `src/workspace.rs`、`src/repo_scan.rs`、`src/probe.rs`：工作区准备、仓库扫描和探测。
- `src/tool_specs.rs`、`src/tool_runtime.rs`、`src/tool_outputs.rs`、`src/tool_context.rs`、`src/tool_adapters/`：工具定义、执行、结构化输出、上下文读取和协议适配。
- `src/dispatch/`、`src/memory/`：执行调度、任务与结果管理，以及贡献记忆。
- `src/paths.rs`、`src/config.rs`、`src/doctor.rs`、`src/inbox.rs`、`src/report.rs`、`src/handoff.rs`、`src/context_pack.rs`：本地路径、配置、诊断、收件箱、报告和交接上下文。
- `tests/`：Rust 集成测试及离线评测 fixtures。
- `README.md`、`README.zh-CN.md`、`docs/README.md`：项目介绍、安装和使用说明。
- `.github/workflows/`：CI 和发布工作流。

## 常用命令

- `cargo build`：编译调试版本，输出到 `target/debug/issue-finder`。
- `cargo run -- doctor`：执行本地就绪检查。
- `cargo run -- tools list`：输出 Codex 的两个 JSON 工具定义和流程元数据。
- `cargo run -- tools --profile session list`：输出当前 agent 会话的 JSON 工具定义。
- `cargo test`：运行 Rust 测试。
- `cargo test --test <name>`：运行指定的 Rust 集成测试。
- `cargo clippy --all-targets -- -D warnings`：执行 lint 检查，将警告视为错误。
- `cargo fmt --all`：格式化 Rust 代码。
- `cargo fmt --all -- --check`：检查 Rust 代码格式。
- `cargo install --path .`：从当前 checkout 安装 CLI。

Rust CLI 默认将本地状态写入 `~/.issue-finder`；设置 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo` 可使用独立的状态目录。

## 测试

Do not write tests for reversible, low-impact changes that mirror the implementation. If you do choose to verify your work with tests, make sure that the tests are meaningful and necessary to verify implementation.

Run tests appropriate to the change and complete required checks. Once those pass, broaden or repeat testing only when new changes, failures, or unresolved concerns justify it; otherwise, continue toward completing the task.

## 解释代码

Use plain language over jargon, and reference technical details only to the degree that it helps illustrate an idea or your work to the user. Communicate complex concepts in a clear and cohesive manner, and calibrate your writing to the level of background knowledge assumed from the user's prompt and context.

## 多代理协作

If at any point you can parallelize work by delegating tasks to another agent (no matter if you are the root or subagent), you should do so using collaboration tools if it could save time or improve quality.

Messages that you send to other agents and your final answer may be read by a human, so ensure they are legible. Always put proper spaces between words and/or numbers.

启动子代理时，使用 `gpt-6.1-sol` 模型和 `high` 推理强度。
