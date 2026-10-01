# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

Issue Finder 面向 Codex 提供 GitHub issue 发现与评估。Codex 负责选择、工作区准备、复现、修复、验证、审查及 PR 交付。

## CLI + skill

[`skills/issue-finder-cli/SKILL.md`](./skills/issue-finder-cli/SKILL.md) 是主要的 CLI 集成入口。当前会话的 tool profile 仅提供 `scout` 和 `assess`，用于发现、过滤、排序和证据评估。不需要 MCP server、dispatch 配置、第二个 agent 会话或交互式 Issue Finder 初始化。

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
```

agent 实际执行命令的环境需要 Git 和兼容的 `issue-finder` 二进制。通过 Cargo 安装已发布的包：

```bash
cargo install issue-finder --locked
```

预编译二进制和校验和见[官方稳定版发布页](https://github.com/lifuyue/issue-finder/releases/latest)。Cloud 环境应在 Codex 启动前落实安装、依赖、PATH 和支持的认证配置。在兼容稳定版发布前，可以明确从当前 checkout 安装：`cargo install --path . --locked`。不能只根据包版本判断兼容性；工具目录必须包含 `sessionContractVersion: 2`：

```bash
issue-finder tools --profile session list
issue-finder tools call issue-finder.scout --arguments '{"limit":5}'
```

GitHub 凭据统一使用环境变量 `GH_TOKEN`，Issue Finder 不再读取 `GITHUB_TOKEN`。没有 `GH_TOKEN` 时，session 工具兼容可选配置中的凭据和宿主已有的 `gh` 登录；提供 `GH_TOKEN` 即可，无需安装 `gh`；无需 `init` 或 status 前置检查。配置、认证和网络错误直接由业务调用返回；读取成功不代表拥有 PR 创建权限。CLI 状态默认位于 `~/.issue-finder`，可用 `ISSUE_FINDER_HOME` 指定独立目录。

GitHub 搜索的排序、查询、分页和 API 预算是明确的工具参数。CLI 对检索到的候选进行贡献价值排序，agent 阅读正文与讨论后选择适合用户的任务。仅推荐的请求会在准备工作区之前结束。所有用户交互都留在当前 agent 会话。工具行为与本地状态见[现状与使用说明](./docs/README.md)。

`tools` 和 `mcp` 默认仅暴露两个 session 工具。评分是评估信息，不是修复授权门槛。Codex 排序忽略历史 `dismissed`、`done` 和 `prepared` 事件，保留自动展示／阅读记录。旧事件和任务文件保留在磁盘，旧任务文件不再读取或续作。本契约不再提供跨聊天手动忽略／恢复或 CLI 完成记录。

## 兼容保留的 CLI 流程

CLI 还提供终端命令、handoff 生成、贡献记忆、JSON/MCP adapter 和独立的 dispatch 控制面。dispatch 管理原生 agent 会话及自身的审批流程；当前会话 skill 不调用它。这些共享兼容路径需要显式旧命令或 `--profile control` / `--profile worker`，不属于 Codex session 契约。详见[使用指南](./docs/README.md)。

## 文档

- [CLI skill](./skills/issue-finder-cli/SKILL.md)
- [现状与使用说明](./docs/README.md)
- [仓库指南](./AGENTS.md)

## 开发

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

隔离运行 CLI 时，设置 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo`。
本仓库基于 [MIT License](./LICENSE) 授权。
