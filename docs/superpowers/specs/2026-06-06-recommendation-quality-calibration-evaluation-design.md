# Recommendation Quality Calibration Evaluation Design

## Status

已批准设计，待执行。

## Background

上一版推荐算法已经引入 recommendation feed、trusted discovery、并发 enrichment 和曝光反馈，但多轮真实运行暴露出稳定质量问题：

- Rust/Go backend profile 在 `limit=15` 时只返回 1 个 visible candidate。
- DevOps/infra profile 在 `limit=15` 时只返回 4 个 visible candidate。
- 65 个真实 visible candidates 中，`profileFit < 70` 占 51%。
- 65 个真实 visible candidates 中，创建超过 1 年的 issue 占 63%，其中不少仍吃到高 freshness。
- 65 个真实 visible candidates 中，`competition_evidence_missing` 占 86%。
- `shown` 反馈只扣 8 分，真实回放中榜首仍保持 rank 1。
- `read` 反馈只扣 35 分，真实回放中默认和 Python profile 榜首仍在首屏。
- GFI trusted 方向正确，但 global fallback 和 weak-profile GFI candidates 仍会污染榜单。

这些问题说明推荐算法不能继续只靠手动查看单次真实运行调参。接下来必须建立完整的离线评测系统，并把每版真实运行结果沉淀为可回归的数据和报告。

## Goals

- 建立稳定的离线 recommendation evaluation system，所有自动测试不依赖真实 GitHub 网络。
- 建立人工标注 fixture dataset，用于验证 ranking、quality policy、feedback cooldown、source trust 和 fallback。
- 建立每版算法固定真实评估 workflow，要求直接读取 top candidates 的 issue 内容和评论来判断价值。
- 将评测链路写入 `AGENTS.md` 和目录级 `README.md`，让后续贡献者和编码代理知道必须如何维护算法评测。
- 实现 fallback C 设计：profile-specific trusted repos 优先，不足再使用 strong profile global query。
- 将推荐质量的最低可见数标准固定为 `visible >= ceil(limit * 0.70)`，目标标准为 `visible >= ceil(limit * 0.80)`。

## Non-Goals

- 不把真实 GitHub API 运行作为 CI 或 `cargo test` 的强制测试。
- 不在 V1 直接修改推荐权重、fallback 行为或 GitHub discovery 逻辑。
- 不复制完整 GitHub API payload 到 fixtures；fixtures 使用 evaluator 所需的最小结构。
- 不提交 GitHub token、临时 `ISSUE_FINDER_HOME`、真实运行缓存或用户私密状态。

## Repository Structure

新增结构：

```text
tests/fixtures/recommendation_eval/
  README.md
  schema.json
  datasets/
    README.md
    core_quality.json
    profile_frontend.json
    profile_backend_rust_go.json
    profile_python_data_cli.json
    profile_ai_agent_tools.json
    profile_devops_infra.json
    source_trust.json
    feedback_replay.json

docs/recommendation-evals/
  README.md
  YYYY-MM-DD-<version>/
    metrics.json
    report.md
    visible.jsonl
```

Evaluator 代码第一版放在测试支持边界内：

```text
tests/support/recommendation_eval.rs
```

如果后续需要提供产品级命令，例如 `issue-finder eval recommendation`，再迁移或抽取到 `src/recommendation/eval.rs`。

## AGENTS.md And README Updates

V1 必须同步更新协作文档。没有这些文档不算完成 V1。

根目录 `AGENTS.md` 新增推荐算法评测章节，要求：

- 修改 discovery、fallback、feed ranking、quality policy、freshness、feedback cooldown 时，必须维护离线评测数据或说明无需维护的原因。
- 自动测试必须使用 `tests/fixtures/recommendation_eval/`，不得依赖真实 GitHub 网络。
- 每个重要算法版本完成后，必须运行 recommendation eval、`cargo test`、`cargo clippy --all-targets -- -D warnings`。
- 每个重要算法版本还必须使用隔离 `ISSUE_FINDER_HOME` 跑固定 6 组真实 profile，并读取 top candidates 的正文和评论评估质量。
- 真实运行结果不作为 CI 强制测试，但应沉淀到 `docs/recommendation-evals/`，并把代表性失败样本补进离线 fixtures。
- GitHub token、临时状态、真实运行缓存不得提交。

`tests/AGENTS.md` 新增 recommendation eval fixture 规则：

- `tests/fixtures/recommendation_eval/` 是推荐算法离线回归集。
- fixtures 必须使用最小结构，不复制完整 GitHub API payload。
- 每个 sample 必须包含 `expected.quality`、`expected.behavior` 和人工 `expected.reasons`。
- 新增算法规则时，应增加能失败旧算法、通过新算法的样本。
- 测试必须 mock 或使用 fixture，不得访问真实 GitHub。

`src/AGENTS.md` 新增 recommendation ranking 边界：

- ranking、feed、quality、freshness、feedback 修改应保持 deterministic。
- 不要在 workflow、tool runtime、daily 中复制 gate 或质量规则。
- fallback discovery 与 feed ranking 分层，避免评分规则散落在 GitHub adapter。
- 修改排序权重时必须同步更新 recommendation eval fixtures 或说明无需更新的原因。

`tests/fixtures/recommendation_eval/README.md` 说明：

- 目录用途。
- sample schema。
- `quality` / `behavior` 标签含义。
- 如何新增样本。
- 如何从真实运行报告转成 fixture。
- 禁止提交 token、完整用户私密数据、临时缓存。

`tests/fixtures/recommendation_eval/datasets/README.md` 说明：

- 每个 dataset 的职责。
- profile 命名规范。
- 每个 dataset 的最低样本量。
- 何时新增 dataset，何时扩展已有 dataset。

`docs/recommendation-evals/README.md` 说明：

- 每版真实评估报告放在哪里。
- 固定 6 组 profile。
- `report.md`、`metrics.json`、`visible.jsonl` 的含义。
- 哪些报告应该提交，哪些只保留在 `/tmp`。
- 如何把失败案例补回 tests fixtures。

## Offline Dataset Design

数据来源分三类。

### Curated Samples

人工挑选和标注的真实 issue 样本，用于覆盖已知缺陷：

- 高质量 clear issue。
- weak profile issue。
- old stale issue。
- old but recently updated issue。
- open PR / claimed / working issue。
- dashboard / Renovate / toy / no-code issue。
- low-impact but overlay trusted issue。
- global noise。
- profile-specific good candidate。

### Live Scout Snapshots

每次真实运行后，将 top candidates 的结构化摘要保存为可审查 snapshot。snapshot 不直接成为网络测试，但可以转化为 curated samples。

### Trusted Source Seed Repos

从 vendored good-first-issue repo list 和 overlay trusted repos 中维护 profile-specific repo buckets：

- `default_cli_devtools`
- `typescript_frontend`
- `rust_backend_systems`
- `python_data_cli`
- `ai_agent_tools`
- `devops_infra`

## Sample Schema

单个 sample 使用最小结构：

```json
{
  "id": "frontend_rjsf_empty_key_good",
  "profile": "typescript_frontend",
  "sourceTier": "gfi_trusted",
  "issue": {
    "repoFullName": "rjsf-team/react-jsonschema-form",
    "number": 5097,
    "title": "additionalProperties: renaming a key to an empty string is silently a no-op",
    "body": "The issue body needed by the evaluator.",
    "labels": ["bug", "help wanted"],
    "createdAt": "2026-06-01T18:08:20Z",
    "updatedAt": "2026-06-01T18:21:23Z",
    "commentsCount": 0
  },
  "repository": {
    "language": "TypeScript",
    "stars": 15797,
    "forks": 2500,
    "openIssues": 179,
    "topics": ["react", "forms", "json-schema"],
    "pushedAt": "2026-06-04T06:09:43Z"
  },
  "comments": [],
  "competition": {
    "openPrRefs": 0,
    "claimComments": 0,
    "workingComments": 0,
    "fixSubmittedComments": 0
  },
  "activity": {
    "maintainerRecentResponse": false,
    "recentIssueActivity": true,
    "recentRepoActivity": true
  },
  "expected": {
    "quality": "excellent",
    "behavior": "visible_top",
    "minProfileFit": 70,
    "maxRankBucket": 3,
    "mustNotHaveRiskTags": ["profile_mismatch", "weak_validation_path"],
    "reasons": [
      "Clear frontend TypeScript bug",
      "Recent issue",
      "No competition evidence against it"
    ]
  }
}
```

`expected.quality` 固定四档：

```text
excellent
good
weak
reject
```

`expected.behavior` 固定为：

```text
visible_top
visible
visible_lower
hidden
fallback_candidate
```

## Dataset Responsibilities

`core_quality.json` 覆盖通用质量问题：

- open PR。
- claimed / working。
- dashboard / Renovate。
- toy / no-code。
- old stale。
- old high freshness。
- overlay trusted 小仓。

`profile_frontend.json` 覆盖 React、UI、browser、form、component。该 dataset 要防止 CDK、CLI、backend-style TypeScript issue 压过真 frontend issue。

`profile_backend_rust_go.json` 覆盖 Rust、Go、backend、compiler、cargo、service。该 dataset 要解决 Rust/Go profile visible 不足问题。

`profile_python_data_cli.json` 覆盖 Python、data、pandas、testing、CLI。该 dataset 要防止 Python profile 被 CDK、Bitwarden、Amplify 等弱相关大仓污染。

`profile_ai_agent_tools.json` 覆盖 AI、LLM、agent、eval、developer tools。该 dataset 要防止教程、bounty、泛 devtools 噪声污染。

`profile_devops_infra.json` 覆盖 Kubernetes、Docker、CI、GitOps、cloud infra。该 dataset 要解决 DevOps visible 不足和 global 噪声问题。

`source_trust.json` 覆盖 GFI trusted、overlay trusted、global 的展示门槛差异。

`feedback_replay.json` 覆盖 shown、read、prepared、done、dismissed、restored 的冷却和恢复行为。

第一版最小完整数据集规模：

```text
profile datasets: each 12-20 samples
core/source/feedback: each 10-15 samples
total: about 100-140 samples
```

## Offline Evaluator

离线 evaluator 只读取 fixtures，不打 GitHub API。

Evaluator 职责：

1. 将 sample 构造成 `EnrichedIssue` / `ValueAssessment` / `RankedValueIssue` 所需输入。
2. 运行当前 deterministic ranking pipeline。
3. 输出 metrics。
4. 生成 JSON 和 Markdown 评估报告。

报告示例：

```json
{
  "dataset": "profile_frontend",
  "samples": 24,
  "precisionAt5": 0.8,
  "precisionAt10": 0.7,
  "visibleFillRate": 0.86,
  "rejectLeakage": 1,
  "profileMismatchLeakage": 2,
  "staleHighRankLeakage": 1,
  "rankingInversions": 3,
  "failures": [
    {
      "sampleId": "frontend_cdk_glue_weak",
      "reason": "weak profile candidate ranked above excellent frontend issue"
    }
  ]
}
```

## Offline Metrics

核心指标：

- `precision@5`
- `precision@10`
- `visibleFillRate`
- `targetVisibleFillRate`
- `rejectLeakage`
- `profileMismatchLeakage`
- `staleHighRankLeakage`
- `competitionLeakage`
- `dashboardNoiseLeakage`
- `rankingInversions`
- `feedbackCooldownPassRate`
- `fallbackFillRate`

第一版 hard pass 门槛：

```text
precision@5 >= 0.75
precision@10 >= 0.65
visibleFillRate >= 0.70
rejectLeakage == 0
dashboardNoiseLeakage == 0
competitionLeakage == 0
profileMismatchLeakage <= 1 per dataset
staleHighRankLeakage <= 1 per dataset
rankingInversions <= 3 per dataset
feedbackCooldownPassRate == 1.0
fallbackFillRate >= 0.70 for weak profiles
```

Target 门槛：

```text
targetVisibleFillRate >= 0.80
```

Target 门槛先作为 warning，不作为 V1 hard fail。

## Ranking Inversion Rules

质量映射：

```text
excellent = 4
good = 3
weak = 2
reject = 0
```

同一个 profile dataset 中：

- `excellent` 不应排在 `weak` 后面。
- `good` 不应排在 `reject` 后面。
- `reject` 不应 visible。
- `fallback_candidate` 在 fallback 场景下应填充 visible 缺口。

以下场景计入 inversion：

- weak profile big repo 排在 excellent profile match 前面。
- old stale high freshness 排在 recent strong match 前面。
- global weak candidate 排在 trusted good candidate 前面。
- low-impact weak validation candidate 排在 clear actionable issue 前面。

## Live Evaluation Workflow

每完成一版重要算法，必须固定跑 6 组真实 profile：

```text
limit = 15
recordExposure = false
refresh = true
```

Profiles:

```text
default_cli_devtools:
  tech_stack = ["Rust", "TypeScript"]
  keywords = ["cli", "developer-tools"]

typescript_frontend:
  tech_stack = ["TypeScript", "JavaScript"]
  keywords = ["frontend", "react", "ui", "browser"]

rust_backend_systems:
  tech_stack = ["Rust", "Go"]
  keywords = ["cargo", "compiler", "performance", "backend"]

python_data_cli:
  tech_stack = ["Python"]
  keywords = ["cli", "data", "pandas", "testing"]

ai_agent_tools:
  tech_stack = ["Python", "TypeScript"]
  keywords = ["ai", "llm", "agent", "developer-tools"]

devops_infra:
  tech_stack = ["Go", "TypeScript"]
  keywords = ["kubernetes", "docker", "ci", "infrastructure"]
```

真实运行 pass/fail：

```text
visible >= ceil(limit * 0.70)
visible >= ceil(limit * 0.80) target
top10 reject/noise/claimed/open PR = 0
top10 profileFit < 60 <= 1
top10 old_but_high_freshness <= 1
```

With `limit = 15`:

```text
hard pass visible >= 11
target pass visible >= 12
```

每版报告输出：

```text
docs/recommendation-evals/YYYY-MM-DD-<version>/
  metrics.json
  report.md
  visible.jsonl
```

报告结构：

1. Version summary。
2. Offline metrics。
3. Live profile table。
4. Top candidate manual review。
5. Failure examples。
6. Regression against previous version。
7. Next optimization priorities。

真实运行结果必须由执行者读取 top candidates 的 issue 正文和评论，并人工标注：

```text
excellent
good
weak
reject
```

代表性失败样本必须补回 offline fixtures。

## Fallback C Design

Fallback C 的目标是在保持质量门槛的前提下，让每组 profile 至少达到 `ceil(limit * 0.70)` visible candidates。

执行顺序：

1. 正常 trusted discovery + enrichment + ranking。
2. 如果 visible candidates 达到 `ceil(limit * 0.70)`，不触发 fallback。
3. 如果不足，先触发 profile-specific trusted repos fallback。
4. 如果仍不足，再触发 strong profile global query fallback。
5. Enrich fallback candidates。
6. Merge、dedupe、rerank。

Profile-specific trusted repos 来源：

- good-first-issue repo list 中按 language、topic、profile terms 预选。
- overlay trusted repos。
- 手动维护 profile repo buckets。

Profile buckets：

```text
default_cli_devtools:
  Rust / TypeScript / Python CLI and developer-tool repos

typescript_frontend:
  React / UI / browser / form / component repos

rust_backend_systems:
  Rust / Go / backend / compiler / cargo / service repos

python_data_cli:
  Python / data / pandas / testing / CLI repos

ai_agent_tools:
  AI / LLM / agent / eval / developer-tool repos

devops_infra:
  Kubernetes / Docker / CI / GitOps / cloud infra repos
```

Strong global query requirements:

```text
is:issue is:open
label:(good first issue OR help wanted OR beginner)
profile terms must match title/body/repo
exclude dashboard/renovate/no-code
updated recently enough OR created recently enough
```

Fallback API budget:

```text
trusted repo fallback:
  max 20 repo issue-list requests
  max 3 candidates per repo

strong global fallback:
  max 8 search requests
  max 30 returned candidates total

fallback enrichment:
  max 40 additional candidates
  reuse existing enrichment concurrency limit
```

Fallback pass goals:

```text
rust_backend_systems visible >= 11 when limit=15
devops_infra visible >= 11 when limit=15
fallbackFillRate >= 0.70
global weak leakage does not increase
```

## Algorithm Roadmap

### V1: Offline Evaluation System And Baseline

V1 不改变 scout 结果，只建立评测系统。

Deliverables:

- `tests/fixtures/recommendation_eval/README.md`
- `tests/fixtures/recommendation_eval/schema.json`
- `tests/fixtures/recommendation_eval/datasets/README.md`
- 初始 datasets。
- `docs/recommendation-evals/README.md`
- 根 `AGENTS.md` 更新。
- `tests/AGENTS.md` 更新。
- `src/AGENTS.md` 更新。
- Baseline offline evaluator。
- Baseline report。

Required checks:

```text
cargo test recommendation_eval
cargo test
cargo clippy --all-targets -- -D warnings
```

### V2: Quality Filtering And Weight Calibration

Goals:

- 消灭明确坏结果。
- 让 profile fit 成为 high-value 前置条件。
- 限制旧 issue 的伪 freshness。
- 改善 shown/read 的冷却行为。

Planned changes:

- open PR / submitted PR / claimed / working 命中 hidden。
- dashboard / Renovate / dependency dashboard / toy no-code hidden。
- `profileFit < 60` hidden 或强降权。
- `profileFit 60-69` 不允许进入 HighValueReady / HighValueNeedsScoping 前排。
- old issue freshness cap。
- High-value rank 从 repo-heavy 调整为 profile-first：

```text
profile_fit       0.35
execution_quality 0.25
repo_influence    0.20
maintainer_signal 0.10
freshness         0.05
risk             -0.25
```

- shown/read 改为冷却或大 penalty。

V2 pass goals:

```text
rejectLeakage == 0
dashboardNoiseLeakage == 0
competitionLeakage == 0
shown feedback does not keep issue at rank 1
read feedback removes issue from first screen
```

### V3: Fallback C

Goals:

- Rust/Go backend 和 DevOps/infra 不再返回过少 candidates。
- 在不扩大 weak global leakage 的前提下，保证 visible fill rate。

Pass goals:

```text
visible >= ceil(limit * 0.70)
target visible >= ceil(limit * 0.80)
fallbackFillRate >= 0.70
```

### V4: Competition Evidence Completion

Goal: 将最终可见候选的 competition evidence 补齐。

Planned changes:

- 当前只对早期 candidates 拉 timeline。
- 改为展示前对 top `2N` 或 top 30 visible-ish candidates 补齐 competition evidence。
- 补齐后重新运行 quality policy 和排序。

API budget:

```text
max competition completion candidates = min(30, limit * 2)
only for visible or high-value candidates missing timeline
```

Pass goals:

```text
competition_evidence_missing in top10 < 20%
open PR / claimed leakage == 0
```

### V5: Performance And Cache

Goal: 降低真实 refresh 成本。

Planned changes:

- Discovery cache 分 lane。
- Enrichment cache 分 source。
- Fallback cache 独立 TTL。
- Live report 复用快照。
- 后台刷新策略，前台优先用 cache。

Pass goals:

```text
refresh=false < 5s
live profile matrix runtime becomes predictable
refresh=true does not grow without bound
```

## Per-Version Workflow

每完成一版算法：

1. 运行离线 evaluator。
2. 运行 `cargo test`。
3. 运行 `cargo clippy --all-targets -- -D warnings`。
4. 使用隔离 `ISSUE_FINDER_HOME` 跑 6 组真实 profile。
5. 读取真实 top candidates 的 issue 正文和评论。
6. 写本版 `docs/recommendation-evals/YYYY-MM-DD-<version>/report.md`。
7. 把代表性失败样本补回 offline fixtures。
8. 基于 metrics 和人工质量判断决定下一版优化点。

## Acceptance Criteria

V1 acceptance:

- 离线 evaluator 可运行，且不访问真实网络。
- 初始 fixtures 覆盖 6 组 profile、source trust、feedback replay 和 core quality。
- 根 `AGENTS.md`、`tests/AGENTS.md`、`src/AGENTS.md` 已更新。
- 新增 README 文档清楚说明 fixtures、datasets 和 live reports 的维护方式。
- Baseline report 已生成。
- `cargo test recommendation_eval` 通过。
- `cargo test` 通过。
- `cargo clippy --all-targets -- -D warnings` 通过。

Full roadmap acceptance:

- 每组 live profile 在 `limit=15` 下至少返回 11 个 visible candidates，目标 12 个。
- top10 中 reject/noise/claimed/open PR 为 0。
- top10 中 `profileFit < 60` 不超过 1。
- top10 中 old high freshness 不超过 1。
- Rust/Go backend 和 DevOps/infra 不再稳定低于 70% visible fill rate。
- representative live failures 会进入 offline fixtures，防止回归。
