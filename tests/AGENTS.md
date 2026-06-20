# 测试目录规范

本文件适用于 `tests/` 下的文件，并在仓库根目录 `AGENTS.md` 的基础上补充测试目录的约定。

## 测试范围

这里主要放集成测试，覆盖用户可见工作流、本地状态布局、GitHub API 行为、workspace 准备流程、JSON tool contract，以及高价值 issue 的本地评分逻辑。测试应聚焦可观察行为，并按被验证的行为命名。

## 隔离规则

测试必须稳定、确定，并且与开发者机器隔离。优先使用 `tempfile`、显式构造的 `IssueFinderPaths`、本地 mock 服务和有作用域的环境变量 guard。不要依赖真实的 `~/.issue-finder`、GitHub、LLM 服务、token、用户 workspace 或外部网络。

## Mock 指南

GitHub 和 LLM 相关工作流优先使用进程内 TCP mock server。响应内容应保持小而稳定，并且只服务当前测试关注的行为。设置 `ISSUE_FINDER_GITHUB_API_BASE` 等环境变量时，必须先获取 `tests/support/env_lock.rs` 中的跨进程锁，并用 guard 在测试结束后清理，避免影响后续测试。

## Tool Contract 测试

Tool contract 测试应覆盖 runtime 入口和 CLI adapter 入口。`tools call` 的 stdout 必须断言为单个 JSON object；`assess` 需要继续断言不写 handoff、inbox 或 workspace state；`prepare` gate 测试应复用 `prepare_gate` 的共享策略，不在测试里复制允许类别。`read_context` 测试必须覆盖非法 section、截断和 symlink/path traversal 防护。

## 高价值 Issue 覆盖

针对高价值 issue 本地算法，优先编写聚焦的评分场景测试，并使用明确的 fixture。时间相关数据使用稳定的相对时间；断言重点放在 `attention_score`、`execution_score`、`recommendation_category`、signals 和 risk tags 行为上。除非测试目标就是阈值边界，否则避免断言脆弱的精确总分。

## Recommendation Evaluation Fixtures

`tests/fixtures/recommendation_eval/` 是推荐算法离线回归集。fixture 必须使用最小结构，不复制完整 GitHub API payload。每个 sample 必须包含 `expected.quality`、`expected.behavior` 和人工 `expected.reasons`，让后续贡献者能理解为什么该 issue 应该上榜、降权或隐藏。

新增或修改 ranking、quality policy、freshness、feedback、source trust 或 fallback 规则时，应增加能失败旧算法、通过新算法的样本。离线 recommendation eval 测试必须使用 fixture、mock 或临时状态，不得访问真实 GitHub。

## Workspace 测试

Workspace 测试应使用临时目录和本地 git 仓库。依赖 git 的测试需要用 `git_available()` 做保护。测试不得修改真实目标仓库，不得在其中安装依赖、提交、推送或创建 PR。

## 常用命令

- `cargo test`：运行全部测试。
- `cargo test --test <name>`：运行指定集成测试文件。
- `cargo fmt --all -- --check`：只检查格式，不重写文件。
- `cargo clippy --all-targets -- -D warnings`：检查所有 target 的 lint。
