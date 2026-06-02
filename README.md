# Patchbay CLI

Patchbay 是一个 local-first 的任务准备工具，面向使用 Codex、Cursor、Claude Code、Cline 等 coding agent 的开发者。

它负责发现合适的 GitHub issue，用本地启发式算法排序，准备仓库工作区，并生成结构化的 handoff 包。后续真正写代码、跑验证、提交 PR 的动作交给用户或 coding agent 完成。

Patchbay 第一版不会修改目标仓库源码，不会自动运行测试，不会 commit、push 或创建 PR。

## 工作流

```text
发现 good first issue
  -> 本地启发式排序
  -> 准备仓库工作区
  -> 生成 handoff.json 和 handoff.md
  -> 写入本地 inbox
  -> 生成 daily report
```

## 环境要求

- Rust toolchain 和 Cargo
- Git
- GitHub Personal Access Token

可选：

- GitHub CLI (`gh`)，用于管理 GitHub token
- OpenAI-compatible API key，用于生成可选的 LLM 摘要

## 安装

从源码构建：

```bash
cargo build
```

使用本地 debug binary：

```bash
target/debug/patchbay --help
```

或从当前 checkout 安装：

```bash
cargo install --path .
patchbay --help
```

## GitHub Token

Patchbay 使用 GitHub REST API 发现 issue 和读取仓库 metadata。

你可以在 `patchbay init` 时填入 token，也可以先通过环境变量提供：

```bash
export GITHUB_TOKEN="$(gh auth token)"
```

本地使用只需要读权限。Patchbay 第一版不需要 GitHub 写权限。

## 快速开始

初始化本地配置和目录：

```bash
patchbay init
```

检查本地环境：

```bash
patchbay doctor
```

发现并排序候选 issue：

```bash
patchbay scout --limit 10
```

准备一个指定 issue：

```bash
patchbay prepare owner/repo#123
```

或使用 GitHub issue URL：

```bash
patchbay prepare --url https://github.com/owner/repo/issues/123
```

查看本地 inbox：

```bash
patchbay inbox
```

查看 handoff Markdown：

```bash
patchbay handoff <inbox-id> --print
```

查看 canonical JSON：

```bash
patchbay handoff <inbox-id> --json
```

运行每日准备流程：

```bash
patchbay daily --top 3
```

查看当天报告：

```bash
patchbay report
```

## 命令一览

| 命令 | 说明 |
| --- | --- |
| `patchbay init` | 创建本地配置和 Patchbay 目录 |
| `patchbay doctor` | 检查 Git、GitHub auth、配置、目录权限和可选 LLM 状态 |
| `patchbay scout --limit 10` | 发现并排序 good-first-issue 候选 |
| `patchbay scout --refresh` | 忽略本地 GitHub issue cache，重新请求 |
| `patchbay prepare owner/repo#123` | 准备一个 issue，并写入 inbox |
| `patchbay prepare --url <url>` | 通过 GitHub issue URL 准备任务 |
| `patchbay handoff <id>` | 显示已有 handoff |
| `patchbay handoff <id> --json` | 输出 canonical `handoff.json` |
| `patchbay inbox` | 查看本地 inbox |
| `patchbay inbox archive <id>` | 标记 inbox item 为 archived |
| `patchbay inbox done <id>` | 标记 inbox item 为 done |
| `patchbay daily --top 3` | scout、准备 Top N issue，并生成日报 |
| `patchbay report` | 显示当天 report |
| `patchbay report --date YYYY-MM-DD` | 显示指定日期 report |

## 本地状态目录

Patchbay 默认把状态存放在 `~/.patchbay`：

```text
~/.patchbay/
  config.toml
  cache/
    github-issues.json
  workspaces/
    owner__repo/
  inbox/
    index.json
    YYYY-MM-DD-owner__repo-123/
      issue.json
      workspace.json
      handoff.json
      handoff.md
  reports/
    YYYY-MM-DD.md
```

测试或隔离运行时，可以用 `PATCHBAY_HOME` 覆盖：

```bash
PATCHBAY_HOME=/tmp/patchbay-demo patchbay doctor
```

## 配置文件

`~/.patchbay/config.toml`：

```toml
[github]
token = ""
username = ""

[profile]
tech_stack = ["Rust", "TypeScript"]
keywords = ["cli", "developer-tools"]

[daily]
top_n = 5

[llm]
enabled = false
base_url = "https://api.openai.com/v1"
api_key = ""
api_key_env = ""
model = "gpt-4o-mini"
```

如果设置了 `llm.api_key_env`，Patchbay 会从对应环境变量读取 LLM key，而不是使用 `llm.api_key`。

## Handoff 输出

`handoff.json` 是 canonical output，包含：

- issue metadata
- workspace path、default branch、Patchbay branch、dirty 状态
- candidate files
- suggested validation commands
- warnings
- 给 coding agent 或用户的 instructions
- 可选 LLM summary 状态

`handoff.md` 是简短的人类可读摘要，并指向 `handoff.json`。

## 安全边界

Patchbay 第一版保持保守。

允许：

- 读取 GitHub issue 和仓库 metadata
- clone 或 fetch 仓库
- 创建或 checkout Patchbay 本地分支
- 在有限范围内扫描仓库文件
- 写入 `~/.patchbay` 下的 Patchbay 本地状态

不允许：

- 修改目标仓库源码
- 自动运行目标仓库验证命令
- 安装依赖
- commit
- push
- 创建 PR
- reset、clean 或删除 workspace

Patchbay 只会把建议的验证命令写入 handoff，不会自动执行。

## 开发

运行测试：

```bash
cargo test
```

运行 clippy：

```bash
cargo clippy --all-targets -- -D warnings
```

格式化：

```bash
cargo fmt --all
```
