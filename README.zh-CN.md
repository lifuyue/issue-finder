# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

Issue Finder 面向 Codex 提供 GitHub issue 发现与评估。Codex 负责选择、工作区准备、复现、修复、验证、审查及 PR 交付。

## CLI + skill

[`skills/issue-finder-cli/SKILL.md`](./skills/issue-finder-cli/SKILL.md) 是主要的 CLI 集成入口。当前会话的 tool profile 仅提供 `scout` 和 `assess`，用于发现、过滤、排序和证据评估。`scout` 在最终选择前，默认使用阿里云原生 `decision-model-preview` 一次回答七个固定语义问题（`scout-semantics-v2`），保留每题各自的材料和标准。筛选前先获取独立的 GitHub 可用性事实，展示前再次获取新事实，并在已有候选池和 API 预算内补足排除项。主 agent 负责实际修复。不需要 MCP server、dispatch 配置或交互式 Issue Finder 初始化。

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

agent 实际执行命令的环境需要 Git、兼容的 `issue-finder` 二进制，以及所选决策模型 provider 的认证配置。通过 Cargo 安装已发布的包：

```bash
cargo install issue-finder --locked
```

预编译二进制和校验和见[官方稳定版发布页](https://github.com/lifuyue/issue-finder/releases/latest)。Cloud 环境应在 Codex 启动前落实安装、依赖、PATH 和支持的认证配置。在兼容稳定版发布前，可以明确从当前 checkout 安装：`cargo install --path . --locked`。不能只根据包版本判断兼容性；工具目录必须包含 `sessionContractVersion: 2`：

```bash
issue-finder tools --profile session list
issue-finder decision-check
issue-finder tools call issue-finder.scout --arguments '{"limit":5}'
```

默认 `aliyun_decision` 需要环境变量 `DASHSCOPE_API_KEY`，以及完整 Workspace System One endpoint，由 `ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT` 或 `[decision.aliyun_decision].endpoint` 指定。`cloudflare_clef_flash` 使用 `CLOUDFLARE_API_TOKEN` 与 `CLOUDFLARE_ACCOUNT_ID`。两个原生接口保留服务端概率。通过 `[decision].provider` 选择 scout provider，使用 `decision-check --provider aliyun-decision`、`--provider cloudflare-clef-flash` 或 `--provider codex` 显式验收某条路径；失败不会静默切换 provider 或开通付费服务。配置、费用与验收边界见 [provider 指南](./docs/decision-providers.md)。新配置使用 `[decision]`；旧 `[system1]` 与 CLI 别名继续兼容，见[命名迁移说明](./docs/decision.md#naming-and-compatibility)。

显式 `codex` 回退使用 `gpt-6-luna`、关闭推理。其 Cloud 准备流程将经过验证的 Codex CLI `0.159.3` 安装到独立 npm prefix；本地只发现并复用已有 CLI 和登录。[仓库安装脚本](./scripts/decision-codex.sh) 支持 `--cloud-install` 和 `--check-only`，两条路径均通过真实模型请求验证结构化输出、模型、关闭推理与认证。详见[安装说明](./skills/issue-finder-cli/references/install.md)。

Codex 回退注入认证时，明确配置 `[decision].provider = "codex"`，将完整原始 Codex `auth.json` 保存为 secret `ISSUE_FINDER_CODEX_AUTH_JSON`，在每个 task 的运行期启动时先运行 `issue-finder decision-auth-init`，再运行 `issue-finder decision-check --provider codex`。可选的非 secret 变量 `ISSUE_FINDER_CODEX_HOME` 指定私有可写的 CLI home，默认使用 Issue Finder 状态目录下的 `system1/codex-home`；只为 Codex 子进程设置该 home。初始化默认保留已有的刷新后文件，`--replace` 显式替换种子；运行时调用不初始化凭据。CLI 刷新不会更新静态 secret，多个并发环境不能共享同一刷新凭据。持久化和验证限制见 [决策模型认证说明](./docs/decision.md#injected-codex-authentication)。

GitHub 凭据统一使用环境变量 `GH_TOKEN`，Issue Finder 不再读取 `GITHUB_TOKEN`。没有 `GH_TOKEN` 时，session 工具兼容可选配置中的凭据和宿主已有的 `gh` 登录；提供 `GH_TOKEN` 即可，无需安装 `gh`；GitHub 无需 `init` 或 status 前置检查。配置、认证和网络错误直接由业务调用返回；读取成功不代表拥有 PR 创建权限。CLI 状态默认位于 `~/.issue-finder`，可用 `ISSUE_FINDER_HOME` 指定独立目录。

GitHub 搜索的排序、查询、分页和 API 预算是明确的工具参数。CLI 对检索到的候选进行贡献价值排序，agent 阅读正文与讨论后选择适合用户的任务。仅推荐的请求会在准备工作区之前结束。所有用户交互都留在当前 agent 会话。

`scout` 默认并发处理四个候选，每个 issue 一次请求保留七题各自的材料；原生接口按 question ID 映射，Codex 回退在一个 app-server 中使用独立 thread。并发配置必须为正数，没有额外上限。单候选失败独立隔离；Codex 仅对可重试响应在原超时内最多重试一次。`scout` 展示语义答案、材料范围、可用性事实、失败和快照；失败或超预算未判断的候选保持可识别，不会静默恢复语义关键词过滤。`assess` 不调用模型，读取当前材料并执行最终层级的事实复核。六小时语义缓存不替代每次获取的新 GitHub 事实。

讨论中的 working、fix_claimed、conflicting 只作证据检查的软提醒，不能直接证明任务被认领或存在 open PR。实际 PR 身份、状态与明确解决关系才构成事实；关闭不等于合并，单纯提及、搜索线索和覆盖不完整保留为未知。文档、生成、活动与奖励任务依据具体目标、清晰度、范围和明确偏好评估，不按任务形式排除。原八题快照和报告仍属历史记录，v1 回放保留当时结果，不用 v2 重算。详见 [决策模型](./docs/decision.md) 与[现状与使用说明](./docs/README.md)。

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
