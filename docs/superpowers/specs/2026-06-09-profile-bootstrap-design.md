# Profile Bootstrap 设计

日期：2026-06-09

状态：已实现

## 摘要

Issue Finder 的推荐质量依赖本地用户画像，但当前 `issue-finder init` 仍要求用户手工填写 `profile.tech_stack` 和 `profile.keywords`。这对主 Agent 使用场景不够开箱即用：Agent 已经能访问用户同意范围内的本地上下文，但 Issue Finder 不应把自己升级成“读懂所有会话并自动画像”的自治系统。

本设计采用半自动方案：新增 `issue-finder profile bootstrap --json`，由 Rust 负责可复现、低风险、完整的数据采集，输出结构化 bootstrap report 和 profile 草案；外置主 Agent 负责审阅证据、去噪、向用户确认，并写入 `~/.issue-finder/config.toml`。安装指引直接在根目录 README 的 Quickstart 中给主 Agent 一段提示词，让首次使用路径更自然。

## 目标

- 新增 `issue-finder profile bootstrap --json`，为主 Agent 提供本地画像初始化材料。
- 完整扫描受支持的 Agent 索引、历史、memory 和项目 manifest 数据源，而不是抽样扫描。
- 默认不读取完整会话正文，不读取系统提示、工具输出、密钥或高风险私密内容。
- 输出结构化 report：活跃项目、技术栈证据、关键词证据、近期任务主题、profile 草案和 warnings。
- 不自动写最终用户画像。主 Agent 必须审阅 report 后再更新 `config.toml`。
- 在根目录 `README.md` 和 `README.zh-CN.md` 的 Quickstart cargo 安装方式之后加入中英文主 Agent 安装/个性化提示词。

## 非目标

- 不让 Rust 自动理解所有 Agent 会话并替用户决定最终画像。
- 不默认读取完整 session transcript。
- 不调用 LLM，不依赖 GitHub，不运行目标项目命令，不安装依赖。
- 不修改目标项目源码、Agent 会话文件或 Issue Finder 之外的状态。
- 不在首版扩展 JSON tool contract。现有 `issue-finder.scout`、`assess`、`prepare`、`read_context` 保持稳定。
- 不把 `profile bootstrap` 做成通用 app bootstrap；它只服务用户画像初始化。

## 命令形态

推荐命令：

```bash
issue-finder profile bootstrap --json
```

原因：

- `profile bootstrap` 清楚表达这是用户画像初始化，而不是整个程序启动流程。
- 后续可以自然扩展 `profile show`、`profile apply` 或 `profile explain`，但首版只实现 bootstrap。
- 安装早期用户未必已经接入 tool adapter，因此不把它作为第一版 tool contract 扩容项。

首版 CLI 行为：

- `--json` 输出单个稳定 JSON object，方便主 Agent 直接读取。
- 没有 `--json` 时可以输出简短人类摘要，但测试和主 Agent 集成以 JSON 为准。
- 默认扫描根是操作系统用户 home，例如 `~/.codex` 和 `~/.claude` 所在目录，不是 `~/.issue-finder`。测试必须通过隔离 home/temp dir 构造数据源。

## 架构

新增模块：

```text
src/profile_bootstrap.rs
```

模块内部保持小边界：

```text
AgentSourceScanner
  -> 发现并解析受支持的 Agent 低风险索引源

ProjectManifestScanner
  -> 从 cwd/project path 读取根目录 manifest

EvidenceAggregator
  -> 聚合技术栈、关键词、活跃项目和任务主题证据

ProfileDraftBuilder
  -> 生成 recommendedProfile 草案和证据引用
```

依赖边界：

- 可以使用本地路径解析辅助，但扫描根必须是操作系统用户 home；`IssueFinderPaths.home` 只用于说明最终 config 位置，不作为 Agent 数据源根目录。
- 不依赖 GitHub、workflow、recommendation engine 或 LLM。
- 不写 `Config`，不调用 `Config::save`。
- 不接触 `prepare_gate.rs`。
- CLI adapter 只负责解析参数、调用 scanner、序列化输出。

## 完整扫描契约

“完整扫描”是首版强约束：

- 对所有声明支持的 Agent 数据源全量遍历，不只读取最近 N 条。
- 对每个 JSONL 文件逐行解析；坏行记录 warning，继续扫描后续行。
- 从索引和 memory 中提取到的 cwd/project path 去重后全量处理。
- 每个项目目录读取所有受支持的根目录 manifest。
- 聚合统计保留完整计数和 source refs；推荐草案可以排序截断，但原始 evidence 不应被抽样丢弃。
- report 明确声明 `fullSupportedSourceScan: true`。

完整扫描不等于默认读取完整会话正文。默认 conversation body mode 必须是 `disabled`。如果后续支持正文读取，必须是显式 opt-in、限量、只抽 user text，并继续排除系统提示、工具输出、密钥和大段私密内容。

## 支持的数据源

首版应支持常见低风险来源，遇到不存在的文件直接跳过：

```text
~/.codex/session_index.jsonl
~/.codex/history.jsonl
~/.codex/memories/*
~/.claude/**/index 或 history 类低风险文件
Cursor 可识别的 workspace/history/memory 索引文件
```

实现时可以先对 `.claude` 和 Cursor 采用保守解析策略：

- 只读取文件名、路径、JSON object 中的 cwd/project/path/title/summary/timestamp 类字段。
- 未知格式不猜测正文结构，记录 warning。
- 不因为单个来源格式变动导致命令失败。

项目 manifest 首版读取根目录文件：

```text
Cargo.toml
package.json
go.mod
pyproject.toml
requirements.txt
pom.xml
build.gradle
settings.gradle
Gemfile
composer.json
```

扫描深度在 report 中标记为：

```text
root_manifest_only
```

这避免把首版误解成全仓库依赖图或全文件内容搜索。

## 输出模型

JSON report 顶层：

```json
{
  "kind": "issue_finder_profile_bootstrap_report",
  "version": 1,
  "scanScope": {
    "agentSources": ["codex", "claude", "cursor"],
    "scanDepth": "root_manifest_only",
    "fullSupportedSourceScan": true,
    "conversationBodyMode": "disabled"
  },
  "agentSources": [],
  "activeProjects": [],
  "techStackEvidence": [],
  "keywordEvidence": [],
  "recentTaskThemes": [],
  "recommendedProfile": {
    "techStack": [],
    "keywords": []
  },
  "warnings": []
}
```

建议字段：

- `agentSources[]`：source kind、path、status、recordsSeen、recordsParsed、warnings。
- `activeProjects[]`：path、firstSeenAt、lastSeenAt、sessionCount、memoryCount、manifestCount。
- `techStackEvidence[]`：term、weight、count、sources、projectRefs、manifestRefs。
- `keywordEvidence[]`：term、weight、count、sources、projectRefs、reason。
- `recentTaskThemes[]`：theme、count、sources、lastSeenAt。
- `recommendedProfile.techStack[]`：按权重排序的短列表。
- `recommendedProfile.keywords[]`：按权重排序的短列表。
- `warnings[]`：机器可读 code、人类可读 message、path 可选。

所有 recommendation 项都应能追溯到 evidence refs。主 Agent 应基于 evidence 去噪，而不是盲写草案。

## Evidence 规则

首版采用可解释启发式，避免引入复杂模型：

- manifest 是强证据。例如 `Cargo.toml` 推断 Rust，`go.mod` 推断 Go，`package.json` 推断 JavaScript/TypeScript 生态。
- 文件名和依赖名是关键词证据。例如 `react`、`vite`、`tokio`、`axum`、`pytest`、`kubernetes`。
- session/memory 的 cwd 频次和最近时间影响项目活跃度。
- session title/summary 类字段可以形成近期任务主题；不能默认读取完整正文补主题。
- 权重应偏向近期、多项目重复出现、manifest 强信号，降低单次偶发路径名的影响。

推荐草案保持简短。实现阶段可设置默认上限：

```text
tech_stack: 8-12 terms
keywords: 12-20 terms
```

如果证据不足，report 仍成功输出，并在 warnings 中提示主 Agent 需要手工补充。

## 错误处理

命令应尽量部分成功：

- JSONL 坏行：记录 warning，继续。
- 单个文件不可读：记录 warning，继续。
- cwd 不存在或不是目录：记录 warning，继续。
- manifest 不支持或解析失败：记录 warning，继续。
- `.claude` 或 Cursor 格式未知：记录 warning，继续。

只有系统级问题才失败：

- 无法确定 home 目录。
- 无法构造扫描根。
- 无法序列化最终 JSON。

## 隐私与安全

默认安全边界：

- 不读取完整会话正文。
- 不读取系统提示。
- 不读取工具输出。
- 不读取 shell 输出、patch、diff 或目标源码内容。
- 不记录环境变量值。
- 不写 Agent session、memory 或目标项目。
- 不写最终 Issue Finder config。

report 里路径可以作为证据出现，因为主 Agent 需要定位项目来源；但实现应避免把大段文件内容写进 report。

## README 安装提示词

实现阶段必须同时更新根目录中英文 README：

- `README.md`
- `README.zh-CN.md`

位置：Quickstart 的 cargo 安装方式之后，也就是 `cargo install issue-finder` 代码块下面、传统 `export GITHUB_TOKEN` / `issue-finder init` 路径之前。

英文 README 加入面向主 Agent 的提示词，表达：

```text
Install cargo issue-finder locally, run `issue-finder profile bootstrap --json`,
review the report's tech stack, keyword, and project evidence, remove noise,
then update `[profile]` in `~/.issue-finder/config.toml`. Do not copy session
bodies, secrets, system prompts, or tool output into the config. Then run
`issue-finder doctor` and `issue-finder scout --limit 10` to verify.
```

中文 README 加入对应提示词，表达：

```text
帮我安装 cargo issue-finder 到本地并启动。请运行
`issue-finder profile bootstrap --json`，审阅报告中的技术栈、关键词和项目证据，
结合我的实际偏好去噪后，更新 `~/.issue-finder/config.toml` 的 `[profile]`。
不要把会话正文、密钥、系统提示或工具输出写进配置。完成后运行
`issue-finder doctor` 和 `issue-finder scout --limit 10` 验证。
```

这段属于 Quickstart 的一部分，不放到深层 usage 文档里才作为主入口。`docs/usage.md` 可以补充字段说明和安全边界，但不能替代根 README 的提示词。

## 测试计划

新增集成测试应使用 `tempfile` 构造 fake home，不读取真实用户目录：

- 通过隔离 HOME 或显式 scan root 构造 fake `.codex` / `.claude` 数据源。
- fake `.codex/session_index.jsonl` 全量扫描多条记录。
- fake `.codex/history.jsonl` 包含坏行，坏行进入 warnings，后续行仍被解析。
- fake memory 文件提供 project path 或任务主题。
- 多个 session 指向同一个 cwd 时 active project 去重并累计计数。
- 每个 fake project 的根目录 manifest 被读取并生成 tech stack evidence。
- `--json` stdout 是单个 JSON object。
- 命令不写 `config.toml`。
- conversation body 默认 disabled。

单元测试覆盖：

- manifest 到 tech stack 的映射。
- evidence 权重和排序。
- warning 构造。
- unknown Agent source 格式的容错。

因为这会影响推荐画像，后续如果改变 `recommendedProfile` 字段或证据权重，应维护 recommendation eval fixtures，或在同次变更中说明为什么无需新增样本。

## 验收标准

- `issue-finder profile bootstrap --json` 在没有任何 Agent 数据源时成功输出空 report 和 warnings。
- 在 fake home 中，所有支持的数据源记录都会被扫描，不抽样。
- report 能解释推荐的 `techStack` 和 `keywords` 来自哪些项目和 manifest。
- 默认不会读取完整会话正文。
- 命令不会自动修改 `~/.issue-finder/config.toml`。
- 根目录英文和中文 README 都在 Quickstart cargo 安装块后提供主 Agent 安装/个性化提示词。
- `cargo test` 和 `cargo clippy --all-targets -- -D warnings` 通过。
