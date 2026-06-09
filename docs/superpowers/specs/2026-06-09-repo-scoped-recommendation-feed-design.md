# Repo-Scoped Recommendation Feed Design

## 背景

当前 `issue-finder scout` 是全局推荐流。Discovery 会从 overlay trusted、profile trusted、Good First Issue trusted repo pool 和 global fallback 中召回候选，再交给 enrichment、value assessment、quality policy、freshness、feedback cooldown 和 competition evidence completion 统一排序。CLI、daily 和 JSON tool contract 都没有指定单一仓库的搜索入口。

用户需要的不是简单的 GitHub issue 列表过滤，而是严格限定在单个仓库内的 Issue Finder 推荐能力：

- `scout --repo owner/repo` 只返回该仓库 issue。
- 仓库内可以 staged recall，但不能跨仓库 fallback。
- 只改变 discovery scope，不绕过主线 value/ranking/quality 体系。
- `daily --repo owner/repo` 和 `issue-finder.scout` tool 也支持同一能力。

## 目标

1. 把指定仓库搜索建模为一等 `DiscoveryScope`，而不是 CLI 或 GitHub adapter 上的临时过滤参数。
2. 为 `DiscoveryScope::Repository` 增加 repo-scoped staged recall，严格只访问目标仓库。
3. 复用主线 recommendation phase：enrichment、value assessment、quality policy、freshness、feedback cooldown、competition evidence completion、feed ranking 和 exposure event。
4. 让 `scout --repo owner/repo --limit 10` 最多返回 10 条该仓库候选，不沿用全局 per-repo display cap。
5. 让 `daily --repo owner/repo --top N` 只从该仓库准备任务，候选不足时不跨仓库补齐。
6. 在 `--stats-json` 和 tool structured output 中暴露 scope 和 repo-scoped stage 诊断，普通文本输出保持简洁。

## 非目标

- 不新增独立 `repo-scout` 命令。
- 不把 repo-scoped feed 做成“列出全部 open issue”的薄包装。
- 不因为用户指定仓库而绕过 prepare gate、quality policy 或 feedback cooldown。
- 不为旧内部函数形状保留多套兼容路径；实现时应清理已过时的参数和 cache key 形状。
- 不在本次设计中改变主线 global scout 的质量模型和排序权重。

## 架构

新增 discovery scope 模型：

```rust
enum DiscoveryScope {
    Global,
    Repository(RepositoryScope),
}

struct RepositoryScope {
    owner: String,
    repo: String,
}
```

`RecommendationEngine::scout`、`workflow::scout_with_options` 和 `daily_candidates` 接收 `DiscoveryScope`。未传 `--repo` 时使用 `DiscoveryScope::Global`，保持现有全局推荐语义；传入 `--repo` 时使用 `DiscoveryScope::Repository`，严格限定仓库。

推荐引擎分成两个清晰阶段：

1. **Discovery phase**：根据 scope 召回 `DiscoveryCandidate`，并记录 lane/stage 诊断。
   - `Global` 继续使用现有 trusted/global staged discovery。
   - `Repository` 使用新的 repo-scoped staged recall，不调用任何跨仓库 fallback。
2. **Recommendation phase**：所有 scope 共用 enrichment、value assessment、quality policy、freshness、feedback cooldown、competition evidence completion、feed ranking 和 exposure recording。

这个边界允许大胆清理旧形状：scope 必须进入 scout result cache key、stats metadata、event metadata 和 tool structured output。不要通过 `Option<String>` 在多层函数中传递 repo 字符串并堆积 if/else。

## Repo-Scoped Staged Recall

Repo scope 的 discovery 只访问 `owner/repo`。所有 lane id 使用 repo 内部命名，便于解释候选来源：

- `repo_scoped:beginner_label:<label>`
- `repo_scoped:help_wanted`
- `repo_scoped:profile_term:<term>`
- `repo_scoped:actionable_keyword:<keyword>`
- `repo_scoped:recent_open`

Repo scope 的 staged recall 以请求的 `limit` 作为可见结果填充目标。每个阶段召回候选后先进入统一 recommendation phase；如果可见结果数量达到 `limit`，停止后续 repo 内 recall；如果不足，进入下一阶段。所有阶段耗尽后仍不足时，返回少于 limit 的结果或空结果。

### Stage 1: 高信号 beginner 标签

对以下标签调用 `/repos/{owner}/{repo}/issues`：

- `good first issue`
- `good-first-issue`
- `beginner`
- `beginner-friendly`
- `easy`
- `starter`

每条 issue 必须是 open、非 PR、未 assigned、未 locked，且不含现有 blocking labels。候选少于 `limit` 时不跨仓库补齐。

### Stage 2: 较宽信号

继续在同一仓库内召回：

- `help wanted`
- profile terms
- actionable keywords，例如 `bug`、`repro`、`expected actual`、`panic`、`error`、`test`

profile/actionable lanes 使用 GitHub issue search，但 query 必须包含 `repo:owner/repo is:issue is:open no:assignee archived:false`，确保 scope 不泄漏到其它仓库。Stage 2 只扩大召回，不提升质量等级；候选是否可展示仍由主线 recommendation phase 决定。

### Stage 3: 自适应 recent open

最近更新 open issue 使用自适应窗口：

1. 先读取最近更新的 100 条 open issue。
2. 如果经过统一 ranking 后可展示结果仍不足，再扩大到 300。
3. 仍不足时扩大到硬上限 500。

每轮只补充未见过的 issue，并继续过滤 PR、assigned、locked 和 blocking labels。Stage 3 不一次性 enrich 全部窗口；它按轮次与 ranking 交替推进，每轮用固定 enrichment budget 选择新增候选，避免大仓库过慢。

## Ranking And Display

Repo scope 的候选进入主线 recommendation phase 后不使用特殊加分或特殊豁免。它们仍然依赖：

- GitHub enrichment 和 enrichment cache。
- `ValueAssessment` 与现有 value gates。
- recommendation quality policy。
- freshness 与 feedback cooldown。
- competition evidence completion。
- prepare gate。

Repo scope 唯一不同的展示规则是取消全局 per-repo display cap。`scout --repo owner/repo --limit 10` 可以返回最多 10 条 `owner/repo` issue。候选不足时结果可以少于 limit 或为空。

## CLI Behavior

新增：

```bash
issue-finder scout --repo owner/repo --limit 10
issue-finder scout --repo https://github.com/owner/repo --limit 10
issue-finder daily --repo owner/repo --top 3
issue-finder daily --repo https://github.com/owner/repo --top 3
```

`--repo` 接受 `owner/repo` 和 GitHub repository URL。它不接受 issue URL；issue URL 仍属于 `assess` 和 `prepare`。格式错误时报：

```text
expected owner/repo or https://github.com/owner/repo
```

`scout --repo` 和 `daily --repo` 都严格限定仓库。仓库不存在或无权限时命令失败并报告 GitHub 404/403，不降级到 global scout。空结果不是错误，普通文本输出保持简洁。

## Tool Contract

`issue-finder.scout` input schema 增加可选 `repo`：

```json
{
  "limit": 10,
  "repo": "owner/repo",
  "refresh": false,
  "includeFiltered": false,
  "recordExposure": true
}
```

省略 `repo` 时保持 global scout。传入 `repo` 时 tool 使用同一个 `DiscoveryScope::Repository`。tool structured output 保持现有 candidates shape，并增加 scope/stage 诊断字段。

## Daily Behavior

`daily --repo owner/repo --top N` 使用 repo-scoped `daily_candidates`。它最多 prepare `N` 个该仓库候选。如果候选少于 `N`，或者 prepare gate 拦截全部候选，daily 报告仍正常写出 prepared/failed/blocked/empty 状态，不从其它仓库补齐。用户指定 repo 不代表允许 bypass prepare gate。

## Output And Diagnostics

普通 `scout --repo` 文本输出沿用主线简洁风格：显示最终可见候选；无结果时只显示短消息，不输出长诊断。

`--stats-json` 和 tool structured output 增加：

- `scope`: `global` 或 `repository`
- `repository`: `owner/repo`，仅 repository scope 有
- `discoveryStages`: 每个 stage/lane 的 requested、returned、deduped、ranked、visible counts
- `stageErrors`: repo 内 lane 的可恢复错误
- `fallbackExhausted`: repository scope 下表示仓库内 staged recall 已耗尽，不表示跨仓库 fallback
- `apiBudget`: 继续沿用现有预算报告
- `filteredCount`: 继续沿用现有含义

Scout result cache key 必须包含 scope、repository、limit、includeFiltered 和 profile fingerprint，避免 global 与 repository scope 互相污染。

## Error Handling

- `--repo` 格式错误直接失败。
- 目标仓库 404/403 直接失败，不 fallback。
- Stage 1 的 repository issues API 如果返回 404/403，整个 repo-scoped scout 失败。
- 单个 Stage 2 search lane 的暂时错误可记录到 `stageErrors` 并继续其它 repo 内 lane。
- Rate limit 使用现有 GitHub budget/error 语义；已取得的 repo 内候选可以继续 ranking，但 stats 必须暴露不完整状态。
- 空结果不是错误。

## Tests

需要覆盖：

- CLI parse：`scout --repo owner/repo`、repo URL、非法 issue URL、`daily --repo owner/repo`。
- Tool schema：`issue-finder.scout` 接受 `repo`，structured output 包含 scope/stage 诊断。
- GitHub mock：repo scope 只请求目标仓库；候选不足时不请求其它 repo 和 global fallback。
- Staged recall：Stage 1 不足触发 Stage 2/3；Stage 1 已满足时不做过深 recent open。
- Recent open 自适应窗口：100 不足再到 300，再到 500 硬上限。
- Ranking 复用：repo-scoped candidates 仍走 enrichment、value assessment、quality policy、feedback cooldown 和 competition completion。
- Display cap：repo scope 下 `limit 10` 可以返回同一仓库超过全局 per-repo cap 的候选。
- Daily：`daily --repo` 只 prepare 目标仓库候选，少于 top 不补齐。
- Cache：global 与 repository scope cache key 互不污染。

## Documentation And Evaluation

实现时更新：

- `docs/usage.md`
- `README.md`
- `README.zh-CN.md`
- tool contract examples if they mention scout inputs

本设计改变 discovery/fallback 行为，但不改变主线 ranking、quality policy、freshness 或 feedback cooldown。若实现只新增 repo scope 并保留 global scout 质量模型不变，可以在变更中说明无需新增离线 recommendation eval fixture。若实现过程中触碰共享 ranking selection、quality policy、fallback thresholds 或 feed ranking，则必须同步维护 `tests/fixtures/recommendation_eval/` 或明确记录无需新增样本的原因。

重要实现完成后应运行：

```bash
cargo fmt --all
cargo test
cargo clippy --all-targets -- -D warnings
```

若共享推荐算法行为发生变化，还应运行离线 recommendation eval 并记录结果。
