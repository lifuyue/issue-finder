# Issue Finder

<p align="center">
  <a href="./README.md">English</a> | <a href="./README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <strong>Issue Finder</strong> 会发现值得交给编码代理处理的 GitHub issue，准备本地上下文，并协调带审批的分派；Issue Finder 自身不修改目标仓库源码。
</p>

<p align="center">
  <img src="./docs/assets/issue-finder-splash.svg" alt="Issue Finder 工作流" width="88%" />
</p>

---

## 快速开始

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

- [**使用指南**](./docs/usage.md)
- [**代理安全的准备运行时**](./docs/agent-safe-preparation-runtime.md)
- [**安全探测**](./docs/safe-probes.md)
- [**历史设计档案**](./docs/superpowers/README.md)
- [**面向编码代理的仓库指南**](./AGENTS.md)

## 开发

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

本仓库基于 [MIT License](./LICENSE) 授权。
