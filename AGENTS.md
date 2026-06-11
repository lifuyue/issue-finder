# 仓库协作指南

## 项目结构与模块划分

本仓库是一个 Rust 2021 命令行程序包。二进制入口在 `src/main.rs`，可复用逻辑从 `src/lib.rs` 导出。命令解析位于 `src/cli.rs`；工作流编排和 issue 选择位于 `src/workflow.rs`；tool contract runtime、结构化输出 DTO 和上下文读取安全边界分别位于 `src/tool_runtime.rs`、`src/tool_outputs.rs` 和 `src/tool_context.rs`；prepare gate 的唯一策略实现位于 `src/prepare_gate.rs`。状态路径、收件箱、报告、GitHub 访问、工作区准备、评分、扫描、配置、诊断检查和大模型支持分别放在职责对应的 `src/*.rs` 模块中。集成测试位于 `tests/`。设计说明位于 `docs/superpowers/specs/`，历史 Rust 设计说明位于 `docs/issue-finder-rust-design.md`。被忽略的 `reference/` 目录是外部参考资料。

## 面向编码代理的设计方向

本项目仍处于足够早期的阶段。当大规模重构能够简化模型、移除陈旧架构，或让工作流更容易理解时，应积极推进。不要仅为了兼容旧的本地版本、旧状态结构或旧命令行为而保留遗留逻辑。优先干净地替换过时路径，避免用最小化补丁不断堆叠新旧并存的设计。

文档也是产品设计的一部分。发现过期文档时，应在同一次变更中更新它，或删除会误导后续贡献者的章节或文件。不要留下过时说明、待办式备注，或相互冲突的新旧描述让之后的贡献者和编码代理自行判断。

## 构建、测试与开发命令

- `cargo build`：编译调试版本二进制，输出到 `target/debug/issue-finder`。
- `cargo run -- doctor`：在本地运行命令行程序并执行就绪检查。
- `cargo test`：运行全部单元测试和集成测试。
- `cargo clippy --all-targets -- -D warnings`：对所有目标执行代码检查，并把警告视为错误。
- `cargo fmt --all`：提交前格式化整个程序包。
- `cargo install --path .`：把当前 checkout 安装为 `issue-finder`。
- `cargo run -- tools list`：本地冒烟验证 JSON tool contract 是否能列出五个 Issue Finder tool specs。

进行隔离的手动运行时，设置 `ISSUE_FINDER_HOME=/tmp/issue-finder-demo`，避免生成的状态写入 `~/.issue-finder`。

## 编码风格与命名约定

使用 `rustfmt`，保持代码符合 Rust 习惯并显式清晰。优先按照既有职责拆分小模块，不要创建宽泛的工具文件。函数、变量、模块和测试名称使用 `snake_case`；结构体、枚举和 trait 使用 `PascalCase`。命令行参数应通过 `clap` 使用清晰的 kebab-case 名称，例如 `--refresh` 或 `--date`。注释保持克制且有用，尤其用于说明安全边界。

## 测试指南

测试使用 Rust 内置测试框架，异步工作流使用 `tokio::test`。面向用户可见工作流、本地状态布局、GitHub 接口行为、工作区准备逻辑和 JSON tool contract 行为，应在 `tests/` 中增加集成测试覆盖。优先使用 `tempfile` 和类似 `ISSUE_FINDER_HOME` 的隔离方式；GitHub 和 tool contract 验证必须使用 mock 或临时状态，不依赖真实网络。测试名称应描述行为，例如 `scout_uses_mocked_github_search_responses`。

## 推荐算法评测与迭代

修改 discovery、fallback、feed ranking、quality policy、freshness 或 feedback cooldown 时，必须同步维护 `tests/fixtures/recommendation_eval/` 中的离线评测数据，或在同次变更中说明为什么无需新增样本。自动测试必须使用 fixture、mock 或临时状态，不得依赖真实 GitHub 网络。

每个重要推荐算法版本完成后，必须运行离线 recommendation eval、`cargo test` 和 `cargo clippy --all-targets -- -D warnings`。重要版本还必须使用隔离的 `ISSUE_FINDER_HOME` 跑固定 6 组真实 profile，并由执行者直接读取 top candidates 的 issue 正文和评论评估价值。真实运行结果不作为 CI 强制测试，但应沉淀到 `docs/recommendation-evals/`，并将代表性失败样本补回离线 fixtures。

不要提交 GitHub token、临时 `ISSUE_FINDER_HOME`、真实运行缓存、生成的用户状态或目标工作区改动。

## 提交与拉取请求指南

近期历史使用简短的祈使句摘要，有时带常规前缀，例如 `Fix daily failure handling and workspace branch checks` 或 `docs: add Issue Finder Rust design`。保持提交聚焦；有帮助时在摘要中提到受影响的工作流。拉取请求应包含简洁描述、已运行的测试、关联议题；只有当生成的 Markdown 或报告相关时才需要截图。

## 安全与配置提示

不要提交令牌、`.env` 文件、生成的 Issue Finder 状态或目标工作区改动。GitHub 和大模型凭据应放在环境变量或 `~/.issue-finder/config.toml` 中。保持项目的保守边界：Issue Finder 可以准备本地工作区并写入交接产物，但不应修改目标仓库源码、安装依赖、提交、推送或创建拉取请求。`issue-finder.prepare` 的默认 gate 只允许高价值类别；不要在 workflow、daily 或 tool adapter 中复制 gate 规则，应复用 `prepare_gate.rs`。
