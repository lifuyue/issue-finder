# 仓库指南

## 当前仓库简介

Issue Finder 用于发现、评估和准备 GitHub issue 贡献任务。仓库包含 Rust 2021 CLI 和独立的 Codex skill 两套实现。

Rust CLI 提供 issue 推荐、评估、工作区准备、handoff、dispatch、贡献记忆以及 JSON tool contract 和 MCP 接口。独立 skill 在当前 Codex 会话中组织发现、选择、实现和验证流程，其 Python helper 提供 `scout`、`prepare`、`finish`，不依赖 Rust CLI、MCP 或 dispatch。

## 项目简要导览

- `skills/issue-finder/`：独立 skill 包，包含 `SKILL.md`、`scripts/issue_finder.py` 和展示元数据 `agents/openai.yaml`。
- `src/main.rs`、`src/lib.rs`、`src/cli.rs`：CLI 入口、库导出和命令行参数定义。
- `src/workflow.rs`、`src/prepare_gate.rs`：工作流编排、issue 选择和 prepare gate 策略。
- `src/recommendation/`、`src/value_*.rs`、`src/scoring.rs`、`src/competition.rs`：推荐排序、价值评估和竞争分析。
- `src/github.rs`、`src/github_enrichment.rs`、`src/discovery.rs`：GitHub 访问、证据补充和候选发现。
- `src/workspace.rs`、`src/repo_scan.rs`、`src/probe.rs`：工作区准备、仓库扫描和探测。
- `src/tool_specs.rs`、`src/tool_runtime.rs`、`src/tool_outputs.rs`、`src/tool_context.rs`、`src/tool_adapters/`：工具定义、执行、结构化输出、上下文读取和协议适配。
- `src/dispatch/`、`src/memory/`：执行调度、任务与结果管理，以及贡献记忆。
- `src/paths.rs`、`src/config.rs`、`src/doctor.rs`、`src/inbox.rs`、`src/report.rs`、`src/handoff.rs`、`src/context_pack.rs`：本地路径、配置、诊断、收件箱、报告和交接上下文。
- `tests/`：Rust 集成测试、独立 skill 的 Python 测试及离线评测 fixtures。
- `README.md`、`README.zh-CN.md`、`docs/skills.md`、`docs/usage.md`：项目介绍、安装和使用说明。
- `docs/recommendation-evals/`：推荐评测记录；`docs/superpowers/`：历史设计文档及索引。
- `.github/workflows/`：CI 和发布工作流。

## 常用命令

- `cargo build`：编译调试版本，输出到 `target/debug/issue-finder`。
- `cargo run -- doctor`：执行本地就绪检查。
- `cargo run -- tools list`：输出 JSON 工具定义和流程元数据。
- `cargo test`：运行 Rust 测试。
- `cargo test --test <name>`：运行指定的 Rust 集成测试。
- `cargo clippy --all-targets -- -D warnings`：执行 lint 检查，将警告视为错误。
- `cargo fmt --all`：格式化 Rust 代码。
- `cargo fmt --all -- --check`：检查 Rust 代码格式。
- `cargo install --path .`：从当前 checkout 安装 CLI。
- `python3 -m unittest discover -s tests -p 'test_skill_native.py' -v`：运行使用 mock GitHub 和临时 Git 仓库的独立 skill 测试，无第三方 Python 依赖。

Rust CLI 默认将本地状态写入 `~/.issue-finder`；设置 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo` 可使用独立的状态目录。

## 测试

Do not write tests for reversible, low-impact changes that mirror the implementation. If you do choose to verify your work with tests, make sure that the tests are meaningful and necessary to verify implementation.

Run tests appropriate to the change and complete required checks. Once those pass, broaden or repeat testing only when new changes, failures, or unresolved concerns justify it; otherwise, continue toward completing the task.

## 解释代码

Use plain language over jargon, and reference technical details only to the degree that it helps illustrate an idea or your work to the user. Communicate complex concepts in a clear and cohesive manner, and calibrate your writing to the level of background knowledge assumed from the user's prompt and context.
