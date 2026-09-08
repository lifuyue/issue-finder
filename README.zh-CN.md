# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <strong>Issue Finder</strong> 帮助你发现并完成值得做的 GitHub issue。本仓库内置独立的 Codex skill，在当前会话中完成发现、准备 workspace、实现和验证。
</p>

---

## 安装与使用 Codex skill

仓库在 [`skills/issue-finder/SKILL.md`](./skills/issue-finder/SKILL.md) 提供了完整、可安装的 skill，同目录包含辅助脚本和 Codex 展示元数据。**本仓库直接提供 skill 集成方式：**将整个目录复制或链接到 Codex 的 skill 目录，然后调用 `$issue-finder`。不需要 Rust 二进制、MCP server、插件、第二个 Codex 会话或 Issue Finder 审批流程。

前置条件：Python 3.9+、Git、[GitHub CLI](https://cli.github.com/) 和支持本地 skill 的 Codex。先完成一次 GitHub 认证：

```bash
gh auth login
gh auth setup-git
```

在**本仓库的 checkout 根目录**中，将整个 skill 目录安装到用户范围：

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/issue-finder "$HOME/.agents/skills/issue-finder"
```

复制命令适用于首次安装。如果目标已存在，应更新已有安装，避免在其中再嵌套一份目录。开发时也可以用绝对路径符号链接代替复制：

```bash
ln -s "$PWD/skills/issue-finder" "$HOME/.agents/skills/issue-finder"
```

如果只想在一个目标仓库中使用，将同一目录复制到 `<目标仓库>/.agents/skills/issue-finder/`。选择一种安装范围，避免重复发现。仓库中的 `skills/` 是分发源码目录，本身不会自动被发现。这些用户级、仓库级路径和符号链接遵循 [Codex 官方 skill 发现约定](https://developers.openai.com/codex/skills/#where-to-save-skills)。安装后若未出现，请重启 Codex。

可选：配置持久的贡献目录：

```bash
export ISSUE_FINDER_WORKSPACE_ROOT="$HOME/Code/contributions"
```

这也是默认目录。请将其放在现有 Git 仓库之外，并确保 Codex 宿主允许写入该目录和访问 GitHub。安装 skill 不会授予 sandbox 权限，也不会安装目标项目依赖。

在 Codex 中调用：

```text
$issue-finder 在 owner/repo 中找一个合适的 issue 并完成它。
$issue-finder 完成 https://github.com/owner/repo/issues/123。
$issue-finder 推荐当前仓库的五个 issue，不要 clone 或修改代码。
$issue-finder 继续 /absolute/path/to/workspace 中已准备的任务。
```

当前会话负责选择、review、实现和验证；内置脚本只有 `scout`、`prepare`、`finish` 三个命令。正常流程不额外要求候选、计划或 review 确认，但仍遵守用户范围、目标仓库规则和宿主权限。commit、push、创建 PR 和发布 GitHub 评论必须有用户授权，辅助脚本自身不会执行这些动作。命令示例、恢复方式与限制见 [skill 集成说明](./docs/skills.md)。

## 现有 Rust CLI

Rust CLI 仍作为现有 handoff/dispatch 实现和迁移对照基线保留。独立 skill 不依赖它，也不会在失败时回退到它。下面的命令及相关架构文档描述的是 Rust CLI。

### 安装并运行 Issue Finder

```bash
cargo install issue-finder
```

配置 GitHub 访问并检查本地就绪状态：

```bash
export GITHUB_TOKEN="$(gh auth token)"
issue-finder init
issue-finder doctor
```

查找候选 issue 并准备交接：

```bash
issue-finder scout --limit 10
issue-finder scout --repo owner/repo --limit 10
issue-finder prepare owner/repo#123
issue-finder handoff <inbox-id> --print
```

Issue Finder 默认将本地状态写入 `~/.issue-finder`。使用 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo` 进行隔离运行。

### 分派与工具契约

Issue Finder 包含带审批的 dispatch 控制面，可管理原生 agent session、A2A task artifact 和 GitHub comment projection。当前命令流程见 [使用指南](./docs/usage.md)。

Issue Finder 也为编码代理暴露 JSON 工具契约：

```bash
issue-finder tools list
```

## 文档

- [**仓库内置的 Issue Finder skill**](./skills/issue-finder/SKILL.md)
- [**Skill 安装、集成与行为**](./docs/skills.md)
- [**使用指南**](./docs/usage.md)
- [**Agent Loop 架构**](./docs/agent-loop-target-architecture.md)
- [**代理安全的准备运行时**](./docs/agent-safe-preparation-runtime.md)
- [**安全探测**](./docs/safe-probes.md)
- [**历史设计档案**](./docs/superpowers/README.md)
- [**面向编码代理的仓库指南**](./AGENTS.md)

## 开发

```bash
python3 -m unittest discover -s tests -p 'test_skill_native.py' -v
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

本仓库基于 [MIT License](./LICENSE) 授权。
