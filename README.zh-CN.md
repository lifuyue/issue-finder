# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

Issue Finder 帮助编码 agent 发现并完成值得做的 GitHub issue。当前 agent 负责与用户交互、选择 issue、实现、验证和汇报。

## CLI + skill

[`skills/issue-finder-cli/SKILL.md`](./skills/issue-finder-cli/SKILL.md) 是主要的 CLI 集成入口。它通过当前会话的 tool profile 提供发现、GitHub 搜索控制、评估、工作区准备、续作、验证和反馈。不需要 MCP server、dispatch 配置、第二个 agent 会话或交互式 Issue Finder 初始化。

将本仓库加入 Codex project，直接按路径调用源码中的 skill：

```text
使用 skills/issue-finder-cli/SKILL.md，推荐五个 Rust CLI 相关的 issue。
使用 skills/issue-finder-cli/SKILL.md，在 owner/repo 中找一个合适的 issue 并完成它。
```

如需在 skill 选择器中发现它，在项目根目录建立链接：

```bash
mkdir -p .agents/skills
ln -s ../../skills/issue-finder-cli .agents/skills/issue-finder-cli
```

如果目标已经存在，应检查并更新现有安装，避免嵌套或覆盖。也可将整个 `skills/issue-finder-cli` 目录复制到 `~/.agents/skills/issue-finder-cli`，供所有项目使用。选择一个发现范围，避免重复。Codex 支持上述仓库级、用户级目录和符号链接；`skills/` 本身是源码包目录，不是自动发现位置。[官方 skill 发现说明](https://learn.chatgpt.com/docs/build-skills#where-to-save-skills)

之后在 Codex 中调用：

```text
$issue-finder-cli 根据我对 Rust 和开发者工具的兴趣，推荐五个 issue。
$issue-finder-cli 在 owner/repo 中找一个合适的 issue 并完成它。
$issue-finder-cli 完成 https://github.com/owner/repo/issues/123。
$issue-finder-cli 继续 /absolute/path/to/workspace 中的任务。
```

agent 实际执行命令的环境需要 Git 和兼容的 `issue-finder` 二进制。通过 Cargo 安装已发布的包：

```bash
cargo install issue-finder --locked
```

预编译二进制和校验和见[官方稳定版发布页](https://github.com/lifuyue/issue-finder/releases/latest)。skill 会检查已安装 CLI 的能力；未安装或不兼容时，会提示安装或升级。在兼容稳定版发布前，可以明确从当前 checkout 安装：`cargo install --path . --locked`。不能只根据包版本判断兼容性；工具目录必须包含 `sessionContractVersion: 1`：

```bash
issue-finder tools --profile session list
issue-finder tools --profile session call issue-finder.status --arguments '{"checkAuth":true}'
```

session 工具依次使用 `GITHUB_TOKEN`、可选配置中的凭据、当前宿主已登录的 `gh`，无需 `init`。需要时先运行 `gh auth login`；私有仓库的 clone/fetch 可通过 `gh auth setup-git` 配置 Git 认证。CLI 状态默认位于 `~/.issue-finder`，可用 `ISSUE_FINDER_HOME` 指定独立目录。

GitHub 搜索的排序、查询、分页和 API 预算是明确的工具参数。CLI 对检索到的候选进行贡献价值排序，agent 阅读正文与讨论后选择适合用户的任务。仅推荐的请求会在准备工作区之前结束。所有用户交互都留在当前 agent 会话。工具约定与恢复流程见 [skill 集成说明](./docs/skills.md) 和 [CLI session 使用指南](./docs/usage.md#current-session-tools)。

## 独立 skill

[`skills/issue-finder/SKILL.md`](./skills/issue-finder/SKILL.md) 保留为独立选项。将整个目录安装或链接到 `~/.agents/skills/issue-finder` 或目标仓库的 `.agents/skills/issue-finder`，然后调用 `$issue-finder`。它需要 Python 3.9+、Git 和已认证的 `gh`。内置 Python helper 提供 `scout`、`prepare`、`finish`，不依赖 Rust CLI、不读取 CLI 状态，也不会回退到 CLI。

```text
$issue-finder 在 owner/repo 中找一个合适的 issue 并完成它。
$issue-finder 推荐当前仓库的五个 issue，不要 clone 或修改代码。
```

它使用较小的粗粒度搜索过滤，不使用 CLI 推荐引擎。两种 skill 都由当前 agent 实现和验证；请明确选择一个 skill，并在同一任务中沿用其流程。详见[独立 skill 行为与命令](./docs/skills.md#standalone-skill)。

## 其他 CLI 流程

CLI 还提供终端命令、handoff 生成、贡献记忆、JSON/MCP adapter 和独立的 dispatch 控制面。dispatch 管理原生 agent 会话及自身的审批流程；当前会话 skill 不调用它。详见[使用指南](./docs/usage.md)。

## 文档

- [CLI skill](./skills/issue-finder-cli/SKILL.md)
- [独立 skill](./skills/issue-finder/SKILL.md)
- [Skill 安装与行为](./docs/skills.md)
- [CLI 使用指南](./docs/usage.md)
- [Dispatch Agent Loop 架构](./docs/agent-loop-target-architecture.md)
- [Handoff 准备运行时](./docs/agent-safe-preparation-runtime.md)
- [历史设计档案](./docs/superpowers/README.md)
- [仓库指南](./AGENTS.md)

## 开发

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
python3 -m unittest discover -s tests -p 'test_skill_native.py' -v
```

隔离运行 CLI 时，设置 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo`。
本仓库基于 [MIT License](./LICENSE) 授权。
