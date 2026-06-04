# 高价值 Gate 排名算法重构设计

日期：2026-06-04

状态：设计已确认，待实现计划

## 摘要

Patchbay 的高价值推荐需要从“多轴分数加风险标签”升级为“先 Gate、再打分、最后按类别排序”。本设计明确高价值的产品语义：高价值不是用户做这个 issue 能学到多少，而是目标仓库有真实影响力，issue 本身符合用户画像，并且适合交给 coding agent 或开发者现在推进。

当前模型已经能识别低深度、无代码、内容填充和部分活动噪音，但这些风险仍然只是线性分数里的降权项。结果是低信任 bounty 仓库、fork 异常增长仓库、竞争 PR 已经饱和的 issue 仍可能靠 high attention 冲到前排。本设计将这些条件改成结构化 gate，确保严重风险不能被单一高分抵消。

## 目标

- 建立高价值推荐的硬边界，避免低信任仓库、低深度填充和饱和竞争任务进入高价值队列。
- 保留低影响力但高度贴合用户画像的小仓库任务，但只能作为 `niche_but_actionable`，不能称为 high value。
- 优化竞争 PR 和 attempt 探测，把大量 closed PR、open PR、claim/attempt 评论作为可做性和维护质量风险。
- 将最终排序改成类别优先，分数只在同类候选中排序。
- 保持 Patchbay 的安全边界：算法只准备 handoff，不修改目标仓库源码、不提交、不推送、不创建 PR。

## 非目标

- 不把 Patchbay 改成自治 coding agent。
- 不基于学习收益定义高价值。
- 不把低深度内容填充任务纳入 high-value gate。
- 不要求 LLM 参与核心 ranking。
- 不为了兼容旧 `agent_ready_high_value` 语义保留旧分类。

## 当前问题

上一版多轴模型使用：

```text
final_rank_score =
  attention_score * 0.55
  + execution_score * 0.30
  + profile_fit_score * 0.10
  - risk_penalty * 0.15
```

这个模型能解释风险，但仍有三个核心缺陷：

1. `attention_score` 过强，可能让 fork 异常、bounty/AI-friendly 标签和短期噪音抵消低信任风险。
2. `risk_penalty` 是软降权，不是硬边界；严重竞争或低影响力仓库仍可能排在真实高价值仓库前面。
3. `profile_fit_score` 仍容易被 repo language/topics 误导，没有充分区分 issue 本体任务和仓库技术栈。

真实样本暴露的问题：

- `beeware/briefcase#2580` 是真实高价值目标，应排在第一。
- `beeware/briefcase#2864` 是真实高价值目标，但范围更大，应排在低风险 ready 目标之后。
- `UnsafeLabs/Bounty-Hunters#917/#918` issue 描述清楚，但仓库 stars 低、fork 异常、bounty/AI-only 噪音高、历史竞争 PR 多，应硬降级。
- `lingdojo/kana-dojo#19461` 是浏览器内 60 秒内容填充任务，应直接进入低深度过滤分类。

## 新核心模型

新算法采用分阶段 pipeline：

```text
raw issue
  -> low-depth preclassification
  -> repo influence gate
  -> competition gate
  -> profile fit gate
  -> execution quality scoring
  -> category policy
  -> within-category rank score
```

原则：

- High value 必须先通过 gate，不能靠分数补回来。
- 低深度填充不进入 high-value gate，直接预分类。
- 仓库影响力是 high value 的硬条件。
- profile fit 看 issue 本体，不只看 repo language/topics。
- 竞争/饱和是硬降级信号。
- 最终类别优先排序，分数只在同类内部排序。

## 输出类别

新推荐类别：

```text
high_value_ready
high_value_needs_scoping
niche_but_actionable
contested_or_low_trust
filtered_low_depth
needs_triage
```

语义：

- `high_value_ready`: 仓库可信且有影响力，issue 与用户画像匹配，可执行性高，竞争不饱和。
- `high_value_needs_scoping`: 仓库可信且有影响力，但 issue 范围较大、证据缺失或需要人工确认边界。
- `niche_but_actionable`: 仓库影响力不足以称为 high value，但 issue 高度贴合用户画像且可执行。
- `contested_or_low_trust`: 仓库低信任、市场噪音高、竞争 PR/attempt 饱和或外部流程异常。
- `filtered_low_depth`: 无代码、微内容、浏览器内快速填充等低工程深度任务。
- `needs_triage`: 信息不足、范围不清、缺少验证、缺少维护者信号或 gate 证据缺失。

默认 CLI 排序：

```text
1. high_value_ready
2. high_value_needs_scoping
3. niche_but_actionable
4. contested_or_low_trust
5. needs_triage
6. filtered_low_depth
```

`daily` 默认只 prepare：

- `high_value_ready`
- 当 ready 不足时，可选择性补 `high_value_needs_scoping`

`contested_or_low_trust` 和 `filtered_low_depth` 默认不 prepare。

## Gate Verdict

每个 gate 输出独立 verdict：

```rust
pub struct GateVerdict {
    pub status: GateStatus,
    pub band: GateBand,
    pub reasons: Vec<String>,
    pub evidence_refs: Vec<String>,
}

pub enum GateStatus {
    Pass,
    SoftFail,
    HardFail,
}

pub enum GateBand {
    Strong,
    Acceptable,
    Weak,
    Suspicious,
    Contested,
    Saturated,
}
```

Gate verdict 必须进入 handoff/report/context 输出，让用户看到分类原因。

## Low-Depth Preclassification

低深度任务不进入 high-value gate。命中后直接分类为 `filtered_low_depth`。

硬信号：

- `no code required`
- `no coding required`
- `no prerequisites needed`
- `do not need to clone`
- `browser in under`
- `<1 minute`
- `under 60 seconds`
- 明确说明手机或浏览器即可完成
- 只添加 JSON 内容、谚语、trivia、grammar point、词条、glossary 或静态内容

这类任务可保留在 scout 末尾或通过 `--include-filtered` 查看，但不得进入 `high_value_*`、`niche_but_actionable` 或 daily prepare。

## Repo Influence Gate

仓库影响力采用 A+C 策略：

- 低影响力或可信度异常仓库保留展示，但硬降级。
- 小仓库只有在 profile fit 极高且任务工程质量好时，才能进入 `niche_but_actionable`。
- 任何小仓库或低信任仓库都不能进入 `high_value_*`。

High-value pass 条件之一：

- `stars >= 1000`
- `stars >= 500 && subscribers >= 20`
- `forks >= 200 && stars >= 500`

Soft pass / niche 条件：

- `stars >= 100`
- `subscribers >= 10`
- `forks >= 50 && fork_star_ratio` 不异常
- 同时 profile fit 极高，issue 本体可执行性高

Suspicious / low-trust 条件：

- `stars < 100`
- `subscribers == 0 && forks` 明显高于 stars
- `fork_star_ratio > 3.0 && stars < 500`
- repo age 小于 90 天且 open issues 很高
- description/topics/labels/body 明显含有 bounty farm、AI-only、agent-only、marketplace queue 等信号

低影响力 gate 失败不意味着完全过滤。它只禁止 high-value 分类。

## Competition Gate

竞争探测采用 B 分层。关闭 PR 不等于不可做，但大量关闭 PR 是强烈的竞争、质量或维护异常信号。

数据来源：

- issue timeline cross-referenced PR
- issue comments
- comment body 中的 `/attempt`、`/claim`、`working on this`、`fix submitted in PR`

Competition points：

```text
open PR reference    = 3
closed PR reference  = 1
/attempt             = 1
/claim               = 1
"working on this"    = 1
"fix submitted in PR"= 1
```

Band：

```text
0-1  => clear
2-3  => light
4-7  => contested
8+   => saturated
```

分类影响：

- `clear` / `light`: 可进入 high-value gate。
- `contested`: 不能进入 `high_value_ready`，最多 `high_value_needs_scoping`，若 repo trust 也弱则进入 `contested_or_low_trust`。
- `saturated`: 直接 `contested_or_low_trust`。

如果 timeline 拉取失败，记录 `competition_evidence_missing`。证据缺失时不能进入 `high_value_ready`，最多进入 `high_value_needs_scoping`。

## Profile Fit Gate

Profile fit 拆成三个正向维度和一个负向维度：

- repo domain fit: 仓库领域是否符合用户偏好，如 CLI、developer tools、backend、web。
- issue task fit: issue 实际任务是否匹配用户技术栈。
- execution environment fit: 用户是否能合理理解、验证或运行该栈。
- negative fit: no-code、content-only、非工程任务直接扣到低 fit。

默认 profile `Rust, TypeScript` + `cli, developer-tools` 的判断：

- Python CLI/devtool issue 可通过 domain fit，例如 BeeWare Briefcase。
- TypeScript repo 中的 JSON 内容填充不能通过 task fit，例如 Kana Dojo proverb。
- Solidity bounty issue 对默认 profile 为 weak fit，除非用户明确配置 crypto/security/Solidity。

## Scores

Gate 之后再打分。分数只负责同类内部排序。

主轴：

```text
repo_influence_score      0-100
profile_fit_score         0-100
execution_quality_score   0-100
maintainer_signal_score   0-100
freshness_score           0-100
risk_score                0-100
```

High-value rank formula：

```text
high_value_rank =
  repo_influence_score * 0.35
  + profile_fit_score * 0.25
  + execution_quality_score * 0.25
  + maintainer_signal_score * 0.10
  + freshness_score * 0.05
  - risk_score * 0.20
```

Niche rank formula：

```text
niche_rank =
  profile_fit_score * 0.40
  + execution_quality_score * 0.35
  + maintainer_signal_score * 0.10
  + repo_influence_score * 0.10
  + freshness_score * 0.05
  - risk_score * 0.20
```

Freshness 只占很小权重，避免当天新建的低深度任务靠“新鲜”排第一。

## Category Policy

分类规则：

```text
if low_depth:
    filtered_low_depth

else if repo_gate pass
  && competition_gate clear/light
  && profile_fit >= 60
  && execution_quality >= 70:
    high_value_ready

else if repo_gate pass
  && competition_gate clear/light/contested
  && profile_fit >= 50
  && execution_quality >= 50:
    high_value_needs_scoping

else if repo_gate soft_fail
  && profile_fit >= 75
  && execution_quality >= 70
  && competition not saturated:
    niche_but_actionable

else if repo_gate suspicious
  || competition saturated
  || marketplace_noise high:
    contested_or_low_trust

else:
    needs_triage
```

对于 `competition_gate == contested`，只有当 repo influence 强、profile fit 和 execution quality 都足够高时，才允许 `high_value_needs_scoping`；否则进入 `contested_or_low_trust`。

## Enrichment Data

现有 enrichment 已包含：

- repo stars
- repo forks
- subscribers/watchers
- open issues
- repo timestamps
- topics/language
- issue body/labels/comments count
- comment excerpts
- recent stargazer sample
- newest fork sample

新增两个事实包：

```rust
pub struct RepoTrustFacts {
    pub fork_star_ratio: f64,
    pub watcher_star_ratio: f64,
    pub repo_age_days: i64,
    pub open_issue_density: f64,
    pub marketplace_terms: Vec<String>,
    pub trust_band: RepoTrustBand,
}

pub struct CompetitionFacts {
    pub open_pr_refs: usize,
    pub closed_pr_refs: usize,
    pub attempt_comments: usize,
    pub claim_comments: usize,
    pub working_comments: usize,
    pub latest_competition_at: Option<String>,
    pub competition_points: i32,
    pub competition_band: CompetitionBand,
    pub warnings: Vec<String>,
}
```

Repo trust facts 主要从已有 repo API 派生，不需要额外 API。

Competition facts 需要 issue timeline API：

```text
GET /repos/{owner}/{repo}/issues/{number}/timeline
Accept: application/vnd.github+json
```

## API Budget

不要对全部搜索结果拉 timeline。

建议流程：

1. GitHub search 后先 cheap rank：profile keywords、repo stars、body low-depth risk。
2. enrich 前 `N=40` 个候选。
3. timeline competition 只对可能进入 `high_value_*` 或 `niche_but_actionable` 的前 `N=20` 个拉。
4. timeline 失败不终止 scout，记录 missing evidence 并限制最高分类。

Cache TTL：

- repo trust 派生数据：6-24 小时。
- issue/comments enrichment：沿用 45 分钟。
- competition timeline：15-30 分钟。

竞争状态变化快，timeline cache 不应过长。

## 模块拆分

本次按大重构处理，不继续把规则堆进 `value_signals.rs`。

建议模块：

```text
src/value_model.rs
  Score axes, gate verdicts, category enums, final public structs.

src/value_gates.rs
  low_depth_preclassification
  repo_influence_gate
  competition_gate
  profile_fit_gate

src/value_scores.rs
  repo_influence_score
  profile_fit_score
  execution_quality_score
  maintainer_signal_score
  freshness_score
  risk_score

src/competition.rs
  GitHub timeline parsing
  attempt/claim/comment pattern detection
  competition band calculation

src/github_enrichment.rs
  Keep API fetching and cache orchestration.
  Add optional competition facts.

src/value_scoring.rs
  Coordinator:
    assess_issue(enriched, profile) -> ValueAssessment
```

`value_signals.rs` 可以缩小为旧模型迁移辅助，也可以被 `value_gates.rs` 和 `value_scores.rs` 逐步替代。

## ValueAssessment 形状

新的 public shape：

```rust
pub struct ValueAssessment {
    pub final_rank_score: i32,
    pub category: RecommendationCategory,
    pub gates: ValueGates,
    pub scores: ValueScores,
    pub risk_tags: Vec<RiskTag>,
    pub evidence: Vec<ValueEvidence>,
    pub missing_evidence: Vec<String>,
}
```

兼容旧字段不是目标。项目仍处早期，可以干净迁移报告、handoff、context pack 和测试。

## 测试矩阵

以 fixture-driven 场景测试作为主保护。

必须覆盖：

1. BeeWare Briefcase path validation bug  
   expected: `high_value_ready`

2. BeeWare Briefcase MSI description bug  
   expected: `high_value_needs_scoping`，因为 issue 明确需要跨 Briefcase 本体和两个 template 仓库协调。

3. Bounty-Hunters high competition bounty  
   expected: `contested_or_low_trust`

4. Kana Dojo no-code JSON proverb  
   expected: `filtered_low_depth`

5. Small Rust CLI repo with excellent issue  
   expected: `niche_but_actionable`

6. High stars but poor issue body  
   expected: `needs_triage`，因为仓库影响力不能替代 issue 本体可执行性。

7. High stars with saturated competition  
   expected: `contested_or_low_trust`

8. Missing timeline evidence  
   expected: cannot be `high_value_ready`

9. TypeScript repo with content-only JSON task  
   expected: weak profile task fit and `filtered_low_depth`

10. Python CLI/devtool issue with explicit reproduction  
    expected: profile fit pass for default CLI/devtool profile

## 当前样本期望

```text
beeware/briefcase#2580        high_value_ready
beeware/briefcase#2864        high_value_needs_scoping
UnsafeLabs/Bounty-Hunters#918 contested_or_low_trust
UnsafeLabs/Bounty-Hunters#917 contested_or_low_trust
lingdojo/kana-dojo#19461      filtered_low_depth
```

## CLI 和报告输出

`scout` 默认输出应按类别分组：

```text
High-value ready
High-value needs scoping
Niche but actionable
Contested or low trust
Needs triage
Filtered low depth
```

每条候选输出：

- category
- rank score
- repo gate summary
- competition gate summary
- profile fit summary
- execution quality summary
- top risk tags
- missing evidence

`--json` 输出完整 gate verdict 和 score axes。

`daily report` 应明确说明跳过原因，例如：

```text
Skipped: contested_or_low_trust
Reason: saturated competition; 7 closed PR refs, 5 attempt comments.
```

handoff/context pack 应展示 gate 证据，避免用户误把低信任任务当成推荐主目标。

## 迁移计划

Phase 1: 新增 value model、gate、score 结构和 fixture tests。  
Phase 2: 新增 competition timeline fetching、parser 和 cache。  
Phase 3: 替换 category policy 和 ranking policy。  
Phase 4: 更新 CLI、report、handoff、context pack 输出。  
Phase 5: 用真实 GitHub 样本重新跑 OpenMeta 对比和 Patchbay live scout。  
Phase 6: 删除或收缩旧 `value_signals` 中不再需要的线性风险逻辑。

## 验收标准

- 当前五个真实样本分类符合“当前样本期望”。
- `filtered_low_depth` 不进入 daily prepare。
- 低影响力仓库即使 issue 可执行，也不能进入 `high_value_*`。
- 小仓库高 fit 高执行任务可以进入 `niche_but_actionable`。
- saturated competition 任务不能进入 `high_value_*`。
- timeline evidence 缺失时不能进入 `high_value_ready`。
- CLI 和 JSON 输出都能解释每个 gate 的 verdict。
- 测试不依赖脆弱的精确总分，优先断言 category、gate band、required risk tags 和 forbidden category。

## 风险和缓解

- GitHub timeline API 增加请求成本。通过两阶段 enrichment、前 20 个候选限制和短 TTL cache 控制。
- Gate 过严可能误伤小而真实项目。通过 A+C 策略保留 `niche_but_actionable`。
- Closed PR 可能只是历史尝试，不一定代表不可做。通过 point-based 分层避免单个 closed PR 直接硬降级。
- 新分类会破坏旧文档和测试。实现时同步更新 README、usage、report、handoff 和 recommendation fixtures。
