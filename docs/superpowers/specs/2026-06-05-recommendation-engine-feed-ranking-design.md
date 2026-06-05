# Recommendation Engine Feed Ranking 设计

日期：2026-06-05

状态：已实现

## 摘要

Issue Finder 的推荐逻辑需要从“高价值 issue 排名”升级为“本地 feed 推荐引擎”。新设计采用约束版 C：统一 `RecommendationEngine` 编排发现、富化、价值评估、反馈状态、时效性和最终排序，但保持两个评分语义严格分离。

- `ValueAssessment` 判断 issue 本身是否高价值、是否适合 prepare。
- `RecommendationAssessment` 判断现在是否应该把该 issue 推给当前用户。

这种设计参考 X/Twitter 公开推荐算法里的 served/seen 降权、时效性和多阶段 ranking 思路，但不照搬具体权重。Issue Finder 的目标不是互动最大化，而是减少重复推荐、提升近期高质量 issue 的优先级，并保护 prepare gate 的安全边界。

## 目标

- 建立统一推荐引擎，让 CLI scout、tool scout 和 daily 共享同一套排序行为。
- 新增 append-only 推荐反馈事件日志，支持 `shown`、`read`、`prepared`、`done`、`dismissed`、`restored`。
- 让已展示、已阅读、已准备的 issue 按次数和时间衰减降权。
- 让 `done` 和 `dismissed` 默认从 feed 隐藏，并支持手动 restore。
- 强化时效性，让近期活跃的高价值候选可以跨相邻高价值类别重排。
- 支持 issue 重新活跃后的排名恢复。
- 保持 `prepare_gate.rs` 的唯一 prepare 策略边界，不用 feed score 绕过 gate。
- 通过 fixture、CLI/tool 集成测试和隔离 live run 对比推荐质量。

## 非目标

- 不把 Issue Finder 改成自治 coding agent。
- 不让推荐反馈改变 issue 的 intrinsic value 判断。
- 不让 `filtered_low_depth` 或 `contested_or_low_trust` 因为新鲜度进入高价值 prepare 队列。
- 不依赖 LLM 做核心排序、反馈解释或 gate 决策。
- 不迁移旧本地状态为新事件日志；新日志从实现后的操作开始积累。
- 不为兼容旧 `scoring.rs` 语义保留最终排序分的旧含义。

## 当前问题

当前代码有三层排序概念分散在不同位置：

1. `src/scoring.rs` 负责 enrichment 前的粗排，但模块名和输出看起来像最终评分。
2. `src/value_scoring.rs` 和 `src/value_scores.rs` 负责高价值 gate、风险和 intrinsic value 分数。
3. `src/workflow.rs` 中的 `sort_by_value` 负责最终排序，并且 CLI/tool/daily 都间接依赖该 workflow 排序。

这种结构导致两个缺口：

- 没有用户反馈层，无法知道某个 issue 是否已经展示、阅读、准备、完成或忽略。
- 时效性只在 `rank_score` 中占很小比例，不能明显影响近期活跃的高质量候选。

本次重构要清理这个边界：`workflow.rs` 不再承载推荐排序策略，最终推荐行为迁移到明确的 recommendation 子系统。

## 核心架构

新增推荐子系统：

```text
src/recommendation/
  mod.rs
  engine.rs
  model.rs
  candidate_ranker.rs
  events.rs
  state.rs
  feedback.rs
  freshness.rs
  feed_ranker.rs
```

推荐流程：

```text
GitHub discovery
  -> cheap candidate prefilter
  -> GitHub enrichment
  -> intrinsic value assessment
  -> feedback state derivation
  -> feed recommendation assessment
  -> final feed ordering
  -> optional exposure recording
```

职责划分：

- `candidate_ranker.rs`：从旧 `scoring.rs` 迁移 enrichment 前 cheap prefilter。它只决定哪些候选值得富化，不代表最终推荐分。
- `engine.rs`：统一编排 discover、enrich、assess、rank、record exposure。
- `events.rs`：读写 append-only 推荐事件日志。
- `state.rs`：从事件日志派生 issue 当前推荐状态。
- `freshness.rs`：计算 issue/repo/activity 的时效性和重新活跃恢复。
- `feedback.rs`：计算 shown/read/prepared 的时间衰减惩罚和 done/dismissed visibility。
- `feed_ranker.rs`：合成 `RecommendationAssessment` 并排序。
- `model.rs`：定义 recommendation 层 DTO。

保留现有模块职责：

- `value_scoring.rs`：继续输出 `ValueAssessment`。
- `value_gates.rs` / `value_scores.rs`：继续服务高价值判断。
- `prepare_gate.rs`：继续只看 `ValueAssessment.recommendation_category`。
- `workflow.rs`：降级为 CLI/tool 工作流编排，调用 `RecommendationEngine`，不再自己实现排序策略。

## 数据模型

Intrinsic value 模型保持现有语义：

```rust
pub struct AssessedIssue {
    pub issue: GitHubIssue,
    pub enriched_issue: EnrichedIssue,
    pub value_assessment: ValueAssessment,
}
```

Feed 推荐模型：

```rust
pub struct RecommendationAssessment {
    pub base_category: RecommendationCategory,
    pub base_rank_score: i32,
    pub freshness_boost: i32,
    pub feedback_penalty: i32,
    pub reactivation_boost: i32,
    pub final_feed_score: i32,
    pub visibility: RecommendationVisibility,
    pub reasons: Vec<String>,
}

pub struct RecommendedIssue {
    pub issue: GitHubIssue,
    pub enriched_issue: EnrichedIssue,
    pub value_assessment: ValueAssessment,
    pub recommendation: RecommendationAssessment,
}
```

Visibility：

```rust
pub enum RecommendationVisibility {
    Visible,
    HiddenDone,
    HiddenDismissed,
    HiddenFiltered,
}
```

规则：

- `ValueAssessment.final_rank_score` 仍是 issue intrinsic value 的同类排名分。
- `RecommendationAssessment.final_feed_score` 是 feed 展示排序分。
- `prepare_gate` 不读取 `final_feed_score`。
- `done` 和 `dismissed` 不改变 `ValueAssessment`，只改变 feed visibility。

## 推荐事件日志

新增 append-only 事件日志：

```text
~/.issue-finder/recommendation/events.jsonl
```

在测试和手动隔离运行时，该路径跟随 `ISSUE_FINDER_HOME`。

事件结构：

```rust
pub struct RecommendationEvent {
    pub event_id: String,
    pub timestamp: String,
    pub issue_key: IssueKey,
    pub event_type: RecommendationEventType,
    pub source: RecommendationEventSource,
    pub issue_updated_at: Option<String>,
    pub issue_comments_count: Option<u64>,
    pub metadata: serde_json::Value,
}

pub struct IssueKey {
    pub repo_full_name: String,
    pub issue_number: u64,
}

pub enum RecommendationEventType {
    Shown,
    Read,
    Prepared,
    Done,
    Dismissed,
    Restored,
}
```

事件来源：

```rust
pub enum RecommendationEventSource {
    CliScout,
    ToolScout,
    CliAssess,
    ToolAssess,
    CliHandoff,
    CliPrepare,
    ToolPrepare,
    InboxDone,
    InboxArchive,
    FeedbackCommand,
    Daily,
}
```

派生状态：

```rust
pub struct RecommendationIssueState {
    pub issue_key: IssueKey,
    pub shown_count: u32,
    pub read_count: u32,
    pub prepared_count: u32,
    pub dismissed: bool,
    pub done: bool,
    pub restored_at: Option<String>,
    pub last_shown_at: Option<String>,
    pub last_read_at: Option<String>,
    pub last_prepared_at: Option<String>,
    pub last_feedback_at: Option<String>,
    pub last_seen_issue_updated_at: Option<String>,
    pub last_seen_comments_count: Option<u64>,
}
```

事件语义：

- `scout` 返回候选后写 `Shown`。
- `assess` 单个 issue 后写 `Read`。
- `handoff` 打开已准备内容后写 `Read`。
- `prepare` 成功后写 `Prepared`。
- `inbox done` 更新 inbox 状态后写 `Done`。
- `inbox archive` 更新 inbox 状态后写 `Dismissed`。
- `feedback read/dismiss/restore` 直接写对应事件。
- `Restored` 让隐藏项重新可见，但不删除历史事件。

不做旧状态迁移。旧 inbox 仍保留；新推荐状态从实现后的事件开始积累。

## Feed Scoring

最终 feed 分：

```text
final_feed_score =
  category_anchor(base_category)
  + base_rank_score
  + freshness_boost
  + reactivation_boost
  - feedback_penalty
```

Category anchor：

```text
high_value_ready          500
high_value_needs_scoping  470
niche_but_actionable      390
needs_triage              300
contested_or_low_trust    180
filtered_low_depth        0
```

这些 anchor 允许 `high_value_needs_scoping` 通过 freshness 和维护者活动超过陈旧的 `high_value_ready`，但不会让低深度或低信任类别冲进高价值池。

排序规则：

```text
1. Visible before hidden items
2. final_feed_score desc
3. base category sort rank
4. base_rank_score desc
5. updated_at desc
```

默认不展示 `HiddenDone`、`HiddenDismissed` 和 `HiddenFiltered`。`include_filtered` 只影响 `HiddenFiltered`，不影响 done/dismissed。

## Freshness 与 Reactivation

Freshness boost：

```text
issue updated <= 24h       +45
issue updated <= 3d        +36
issue updated <= 7d        +28
issue updated <= 14d       +18
issue updated <= 30d       +10
older                      +0
```

高质量活动额外加分：

```text
maintainer_recent_response +20
recent_repo_activity       +8
recent_issue_activity      +8
```

Reactivation：

```text
issue.updated_at > last_feedback_at
  -> reactivation_boost +15
  -> feedback_penalty *= 0.70

comments_count > last_seen_comments_count
  -> reactivation_boost +25
  -> feedback_penalty *= 0.50

maintainer_recent_response after feedback
  -> reactivation_boost +35
  -> feedback_penalty *= 0.35
```

第一版如果无法精确判断 maintainer response 是否发生在 feedback 之后，使用 `maintainer_recent_response && updated_at > last_feedback_at` 作为近似，并在 reason 中明确说明。

`done` 和 `dismissed` 默认仍隐藏。重新活跃不会自动解除隐藏，用户需要 `feedback restore`。

## Feedback Penalty

反馈惩罚采用次数叠加和时间衰减：

```text
shown penalty    = 8  * decay(age) * min(shown_count, 5)
read penalty     = 35 * decay(age) * min(read_count, 3)
prepared penalty = 80 * decay(age) * min(prepared_count, 2)
done/dismissed   = hidden
```

Decay 阶梯：

```text
age <= 1d    1.00
age <= 3d    0.75
age <= 7d    0.50
age <= 14d   0.25
older        0.10
```

`age` 使用对应事件类型的最近时间：

- shown 使用 `last_shown_at`
- read 使用 `last_read_at`
- prepared 使用 `last_prepared_at`

最终 penalty 四舍五入为整数，并进入 recommendation reasons。

## CLI 行为

`issue-finder scout` 默认记录曝光：

```text
issue-finder scout --limit 20
```

行为：

- 返回候选后写 `Shown`。
- 输出包含 feed score、base category、freshness、feedback penalty、reactivation 和主要 reasons。

新增只读逃生口：

```text
issue-finder scout --dry-run
```

`--dry-run` 不写 `Shown`，用于测试、调试和 baseline 对比。

`assess`：

- CLI assess 单 issue 后写 `Read`。
- 输出包含 value assessment 和 recommendation assessment。

`handoff`：

- `--json` 和 `--print` 都写 `Read`。

`prepare`：

- 成功后写 `Prepared`。
- gate 阻止时不写 `Prepared`，但写 `Read`，因为调用方已显式评估该 issue。

`inbox`：

```text
issue-finder inbox done <id>
issue-finder inbox archive <id>
```

- `done` 写 `Done` 事件。
- `archive` 写 `Dismissed` 事件。

新增顶级反馈命令：

```text
issue-finder feedback read owner/repo#123
issue-finder feedback dismiss owner/repo#123
issue-finder feedback restore owner/repo#123
issue-finder feedback show owner/repo#123
```

`feedback show` 显示派生状态和最近事件，用于解释降权或隐藏原因。

## Tool Runtime 行为

`issue-finder.scout` 也默认记录曝光，并新增参数：

```json
{
  "recordExposure": true
}
```

传入 `recordExposure=false` 时只读。

`issue-finder.assess`：

- 默认写 `Read`。
- 新增 `recordRead`，默认 `true`。

`issue-finder.prepare`：

- 成功后写 `Prepared`。
- 被 gate 阻止时不写 `Prepared`，但写 `Read`。

Tool output 中的 candidate 和 assessment 增加 recommendation 字段：

```json
{
  "recommendation": {
    "baseCategory": "high_value_ready",
    "baseRankScore": 78,
    "freshnessBoost": 45,
    "feedbackPenalty": 16,
    "reactivationBoost": 0,
    "finalFeedScore": 607,
    "visibility": "visible",
    "reasons": []
  }
}
```

Tool adapter 只负责解析参数和序列化输出，不重新实现 ranking、feedback 或 gate 规则。

## Daily 行为

`daily` 使用 `RecommendationEngine` 的 feed 排序，但仍遵守 prepare gate：

- 不 prepare hidden done/dismissed。
- 不 prepare filtered/contested/low-trust。
- 不因为 feed score 高而绕过 `prepare_gate`。
- 对实际 prepare attempt 的候选写 `Shown`。
- 成功 prepare 后写 `Prepared`。

Daily 不给“只被内部排序但没有展示、没有尝试 prepare”的候选写 `Shown`。后台自动流程不应把用户没看到的候选当成刷过。

## Handoff、Report 与 Context 输出

handoff 继续保存 `value_assessment`。通过 recommendation engine 准备 issue 时，第一版必须同时保存 `recommendation`：

```json
{
  "value_assessment": {},
  "recommendation": {}
}
```

报告和 context 中应区分：

- Value: 为什么这个 issue 本身值得做。
- Recommendation: 为什么现在推荐或不推荐给用户。

输出中不得把 feed 降权描述成 issue 价值下降。例如应写：

```text
Shown 3 times in the last 7 days, so feed ranking was reduced.
```

而不是：

```text
Issue quality decreased because it was shown before.
```

## 测试策略

### Deterministic Fixture 测试

新增 recommendation fixture，覆盖：

- 多次 `shown` 后下一轮排序下降。
- `read` 比 `shown` 降权更强。
- `prepared` 强降权，避免重复 prepare。
- `done` 和 `dismissed` 默认隐藏。
- `restore` 后重新可见。
- `updated_at > last_feedback_at` 小恢复。
- `comments_count` 增加大恢复。
- 维护者近期响应带来更强恢复。
- 近期 `high_value_needs_scoping` 可超过陈旧 `high_value_ready`。
- `filtered_low_depth` 和 `contested_or_low_trust` 不能靠 freshness 进入高价值区。

### CLI / Tool 集成测试

覆盖：

- `scout` 默认写 `Shown`。
- `scout --dry-run` 不写事件。
- tool `issue-finder.scout` 默认写 `Shown`。
- tool scout `recordExposure=false` 不写事件。
- `assess` 写 `Read`。
- `prepare` 成功写 `Prepared`。
- `inbox done` 写 `Done`。
- `inbox archive` 写 `Dismissed`。
- `feedback read/dismiss/restore/show` 可作用于未进入 inbox 的 issue。

所有测试使用 `tempfile` 或 `ISSUE_FINDER_HOME` 隔离。GitHub 行为使用 mock 或固定 enriched issue，不依赖真实网络。

### Isolated Live Run 对比

手动验证使用独立状态目录：

```bash
ISSUE_FINDER_HOME=/tmp/issue-finder-baseline cargo run -- scout --dry-run --limit 10
ISSUE_FINDER_HOME=/tmp/issue-finder-feed cargo run -- scout --limit 10
ISSUE_FINDER_HOME=/tmp/issue-finder-feed cargo run -- scout --limit 10
ISSUE_FINDER_HOME=/tmp/issue-finder-feed cargo run -- feedback show owner/repo#123
```

比较重点：

- 第二轮是否减少重复展示。
- 近期高质量 issue 是否上升。
- dismissed/done 是否消失。
- restore 后是否恢复。
- 有新 activity 的 issue 是否恢复部分排名。
- daily 是否仍只 prepare gate 允许的类别。

## 实施顺序

1. 新建 `src/recommendation/` 模块和数据模型。
2. 从 `scoring.rs` 迁移 cheap candidate ranker。
3. 实现 events log 和 state derivation。
4. 实现 freshness、feedback、reactivation 和 feed ranker。
5. 改 `workflow::scout`、`daily`、`assess`、`prepare` 调用 engine。
6. 改 CLI 参数和新增 `feedback` 命令。
7. 改 tool runtime schema 和 output DTO。
8. 更新 report、handoff、context 中的 recommendation explanation。
9. 增加 fixture、CLI、tool contract 测试。
10. 更新或删除旧排序语义文档，避免新旧描述冲突。

## 设计约束

- `prepare_gate.rs` 是 prepare 策略唯一来源。
- `workflow.rs` 不复制 feed 排序、feedback 惩罚或 prepare gate 规则。
- CLI 和 tool runtime 必须共享 `RecommendationEngine`。
- JSON output 中同时暴露 value assessment 和 recommendation assessment，避免消费者混淆。
- append-only event log 是推荐反馈事实来源，不从 Markdown/report 反推状态。
- 本地状态写入必须服从 `ISSUE_FINDER_HOME`。
- 网络相关测试必须 mock，不能依赖真实 GitHub。
