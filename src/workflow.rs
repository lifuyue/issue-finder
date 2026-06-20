use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config::Config;
use crate::evidence_pack::{build_evidence_pack, EvidencePack};
use crate::github::{GitHubClient, GitHubIssue, IssueRef};
use crate::handoff::{handoff_id, write_handoff_with_events, Handoff, WrittenHandoff};
use crate::inbox;
use crate::llm;
use crate::llm_review;
use crate::memory::handoff_memory_context_for_issue;
use crate::paths::IssueFinderPaths;
use crate::prepare_events::PrepareEventLog;
use crate::prepare_gate::default_prepare_allowed;
use crate::probe::SafeProbeRunner;
use crate::readiness::assess_readiness;
use crate::recommendation::{
    load_state_map, recent_events_for_issue, record_event_for_issue, record_event_for_key,
    DiscoveryScope, IssueKey, RecommendationEngine, RecommendationEventSource,
    RecommendationEventType, ScoutOptions, ScoutResult,
};
use crate::report::{self, DailyReport, FailedReportItem, PreparedReportItem};
use crate::value_scoring::RankedValueIssue;
use crate::workspace;

const ENRICHED_SCOUT_CANDIDATE_LIMIT: usize = 40;

#[derive(Debug, Clone)]
pub enum PrepareOutcome {
    Prepared(Box<PreparedReportItem>),
    Failed(FailedReportItem),
}

#[derive(Debug, Clone, Default)]
pub struct PrepareOptions {
    pub explicit_prepare: bool,
    pub gate_bypass_reason: Option<String>,
    pub recommendation_source: Option<RecommendationEventSource>,
}

#[derive(Debug, Clone, Default)]
pub struct IssueSelector {
    pub issue: Option<String>,
    pub url: Option<String>,
}

impl IssueSelector {
    pub fn new(issue: Option<String>, url: Option<String>) -> Self {
        Self { issue, url }
    }

    pub fn issue_ref(&self) -> Result<IssueRef> {
        match (
            normalize_optional(&self.issue),
            normalize_optional(&self.url),
        ) {
            (Some(issue), None) => IssueRef::parse(&issue),
            (None, Some(url)) => IssueRef::parse_url(&url),
            (Some(_), Some(_)) => {
                anyhow::bail!("pass either an issue reference or --url, not both")
            }
            (None, None) => {
                anyhow::bail!(
                    "pass owner/repo#123 or --url https://github.com/owner/repo/issues/123"
                )
            }
        }
    }
}

fn normalize_optional(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub async fn scout(
    paths: &IssueFinderPaths,
    config: &Config,
    limit: usize,
    refresh: bool,
) -> Result<Vec<RankedValueIssue>> {
    Ok(scout_with_options(
        paths,
        config,
        limit,
        refresh,
        ScoutOptions::cli(),
        DiscoveryScope::Global,
    )
    .await?
    .ranked)
}

pub async fn scout_with_options(
    paths: &IssueFinderPaths,
    config: &Config,
    limit: usize,
    refresh: bool,
    options: ScoutOptions,
    scope: DiscoveryScope,
) -> Result<ScoutResult> {
    RecommendationEngine::new(paths, config)
        .scout(limit, refresh, options, scope)
        .await
}

pub async fn assess_issue_selection(
    paths: &IssueFinderPaths,
    config: &Config,
    selector: IssueSelector,
    refresh: bool,
) -> Result<RankedValueIssue> {
    assess_issue_selection_with_options(
        paths,
        config,
        selector,
        refresh,
        true,
        RecommendationEventSource::CliAssess,
    )
    .await
}

pub async fn assess_issue_selection_with_options(
    paths: &IssueFinderPaths,
    config: &Config,
    selector: IssueSelector,
    refresh: bool,
    record_read: bool,
    source: RecommendationEventSource,
) -> Result<RankedValueIssue> {
    paths.ensure_layout()?;
    let reference = selector.issue_ref()?;
    let github = GitHubClient::new(config)?;
    let issue = github.fetch_issue(&reference).await?;
    RecommendationEngine::new(paths, config)
        .assess_issue(issue, refresh, record_read, source)
        .await
}

pub async fn prepare_from_input(
    paths: &IssueFinderPaths,
    config: &Config,
    issue: Option<String>,
    url: Option<String>,
) -> Result<PrepareOutcome> {
    let ranked = assess_issue_selection_with_options(
        paths,
        config,
        IssueSelector::new(issue, url),
        false,
        true,
        RecommendationEventSource::CliPrepare,
    )
    .await?;
    prepare_value_issue_with_options(
        paths,
        config,
        ranked,
        PrepareOptions {
            explicit_prepare: true,
            gate_bypass_reason: None,
            recommendation_source: Some(RecommendationEventSource::CliPrepare),
        },
    )
    .await
}

pub async fn prepare_issue(
    paths: &IssueFinderPaths,
    config: &Config,
    issue: GitHubIssue,
) -> Result<PrepareOutcome> {
    let ranked = RecommendationEngine::new(paths, config)
        .assess_issue(issue, false, true, RecommendationEventSource::CliPrepare)
        .await?;
    prepare_value_issue_with_options(
        paths,
        config,
        ranked,
        PrepareOptions {
            explicit_prepare: true,
            gate_bypass_reason: None,
            recommendation_source: Some(RecommendationEventSource::CliPrepare),
        },
    )
    .await
}

pub async fn prepare_value_issue_with_options(
    paths: &IssueFinderPaths,
    config: &Config,
    ranked: RankedValueIssue,
    options: PrepareOptions,
) -> Result<PrepareOutcome> {
    let issue = ranked.issue.clone();
    let event_path = paths
        .inbox_item_dir(&handoff_id(&issue))
        .join("prepare-events.jsonl");
    let (events, event_warning) = match PrepareEventLog::create(&event_path) {
        Ok(events) => {
            let _ = events.append_prepare_started(&issue);
            (Some(events), None)
        }
        Err(error) => (
            None,
            Some(format!("Unable to initialize prepare event log: {error}")),
        ),
    };
    if let (Some(events), Some(reason)) = (&events, &options.gate_bypass_reason) {
        let _ = events.append(
            "prepare_gate_bypassed",
            &[
                (
                    "category",
                    Value::String(ranked.value_assessment.recommendation_category.to_string()),
                ),
                ("reason", Value::String(reason.clone())),
            ],
        );
    }

    match workspace::prepare_workspace(paths, &issue) {
        Ok(workspace) => {
            let mut workspace = workspace;
            if let Some(warning) = event_warning {
                workspace.warnings.push(warning);
            }
            if let Some(reason) = &options.gate_bypass_reason {
                workspace
                    .warnings
                    .push(format!("Prepare gate bypass: {reason}"));
            }
            if let Some(events) = &events {
                let _ = events.append(
                    "workspace_prepared",
                    &[
                        ("path", Value::String(workspace.info.path.clone())),
                        ("branch", Value::String(workspace.info.branch.clone())),
                    ],
                );
            }
            if options.explicit_prepare && ranked.value_assessment.execution_score < 40 {
                workspace.warnings.push(format!(
                    "Explicit prepare bypassed low execution score {}",
                    ranked.value_assessment.execution_score
                ));
            }
            let probe_pack = SafeProbeRunner::default().run(
                Path::new(&workspace.info.path),
                &workspace.scan,
                events.as_ref(),
            );
            let readiness = assess_readiness(&issue, &workspace, &probe_pack);
            let evidence_pack = build_evidence_pack(
                &ranked.value_assessment,
                &ranked.enriched_issue,
                Some(&workspace.scan),
            );
            let llm_review = llm_review::review_handoff(
                config,
                &issue,
                &ranked.value_assessment,
                &evidence_pack,
            )
            .await;
            let mut handoff = Handoff::build_with_recommendation(
                &issue,
                &workspace,
                ranked.value_assessment.clone(),
                ranked.recommendation.clone(),
                evidence_pack.clone(),
                llm_review,
            );
            handoff.probe_pack = probe_pack;
            handoff.readiness = readiness;
            handoff.memory_context =
                handoff_memory_context_for_issue(paths, &issue).unwrap_or_default();
            llm::enhance_handoff(config, &mut handoff).await;
            let written = match write_handoff_with_events(paths, &handoff, &issue, events.as_ref())
            {
                Ok(written) => written,
                Err(error) => {
                    if let Some(events) = &events {
                        let _ = events.append(
                            "prepare_failed",
                            &[("reason", Value::String(error.to_string()))],
                        );
                    }
                    return Err(error);
                }
            };
            let source = options
                .recommendation_source
                .unwrap_or(if options.explicit_prepare {
                    RecommendationEventSource::CliPrepare
                } else {
                    RecommendationEventSource::Daily
                });
            let _ = record_event_for_issue(
                paths,
                &issue,
                Some(&ranked.enriched_issue),
                RecommendationEventType::Prepared,
                source,
                serde_json::json!({ "handoffId": written.id }),
            );
            inbox::upsert_ready(paths, &issue, ranked.score, &written)?;
            Ok(PrepareOutcome::Prepared(Box::new(prepared_report_item(
                &ranked,
                &evidence_pack,
                &handoff,
                &written,
            ))))
        }
        Err(error) => {
            let reason = error.to_string();
            if let Some(events) = &events {
                let _ = events.append(
                    "prepare_failed",
                    &[("reason", Value::String(reason.clone()))],
                );
            }
            inbox::upsert_prepare_failed(paths, &issue, ranked.score, reason.clone())?;
            Ok(PrepareOutcome::Failed(FailedReportItem {
                repo_full_name: issue.repo_full_name,
                issue_number: issue.number,
                title: issue.title,
                score: ranked.score,
                reason,
            }))
        }
    }
}

pub async fn daily(
    paths: &IssueFinderPaths,
    config: &Config,
    top: Option<usize>,
    refresh: bool,
    scope: DiscoveryScope,
) -> Result<(DailyReport, String)> {
    paths.ensure_layout()?;
    let top_n = top.unwrap_or(config.daily.top_n).max(1);
    let result = RecommendationEngine::new(paths, config)
        .daily_candidates(refresh, ENRICHED_SCOUT_CANDIDATE_LIMIT, scope)
        .await?;

    daily_from_ranked(paths, config, result.ranked, result.discovery_count, top_n).await
}

pub async fn daily_from_ranked(
    paths: &IssueFinderPaths,
    config: &Config,
    ranked: Vec<RankedValueIssue>,
    discovery_count: usize,
    top_n: usize,
) -> Result<(DailyReport, String)> {
    let mut report = report::empty_report(discovery_count);
    let mut attempts = 0usize;

    for ranked_issue in ranked {
        if attempts >= top_n {
            break;
        }
        if !ranked_issue.recommendation.displayable(false) {
            continue;
        }
        if inbox::contains_issue(
            paths,
            &ranked_issue.issue.repo_full_name,
            ranked_issue.issue.number,
        )? {
            continue;
        }
        if !default_prepare_allowed(ranked_issue.value_assessment.recommendation_category) {
            continue;
        }

        attempts += 1;
        let issue = ranked_issue.issue.clone();
        let _ = record_event_for_issue(
            paths,
            &issue,
            Some(&ranked_issue.enriched_issue),
            RecommendationEventType::Shown,
            RecommendationEventSource::Daily,
            serde_json::json!({ "reason": "daily_prepare_attempt" }),
        );
        match prepare_value_issue_with_options(
            paths,
            config,
            ranked_issue.clone(),
            PrepareOptions {
                explicit_prepare: false,
                gate_bypass_reason: None,
                recommendation_source: Some(RecommendationEventSource::Daily),
            },
        )
        .await
        {
            Ok(PrepareOutcome::Prepared(item)) => report.prepared.push(*item),
            Ok(PrepareOutcome::Failed(item)) => report.failed.push(item),
            Err(error) => {
                let reason = error.to_string();
                let _ =
                    inbox::upsert_prepare_failed(paths, &issue, ranked_issue.score, reason.clone());
                report.failed.push(FailedReportItem {
                    repo_full_name: issue.repo_full_name,
                    issue_number: issue.number,
                    title: issue.title,
                    score: ranked_issue.score,
                    reason,
                });
            }
        }
    }

    let report_path = report::write_daily_report(paths, &report)?;
    Ok((report, report_path))
}

pub fn read_handoff(paths: &IssueFinderPaths, id: &str, json: bool) -> Result<String> {
    let item = inbox::find_item(paths, id)?;
    let _ = record_event_for_key(
        paths,
        IssueKey::new(item.repo_full_name.clone(), item.issue_number),
        RecommendationEventType::Read,
        RecommendationEventSource::CliHandoff,
    );
    let path = if json {
        item.handoff_json_path
    } else {
        item.handoff_md_path
    };
    if path.trim().is_empty() {
        anyhow::bail!("inbox item {id} has no handoff file");
    }

    fs::read_to_string(&path).with_context(|| format!("unable to read {path}"))
}

pub fn read_report(paths: &IssueFinderPaths, date: Option<String>) -> Result<String> {
    let date = date.unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let path = paths.report_path(&date);
    fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))
}

pub fn record_feedback(
    paths: &IssueFinderPaths,
    issue: &str,
    event_type: RecommendationEventType,
) -> Result<String> {
    paths.ensure_layout()?;
    let reference = IssueRef::parse(issue)?;
    let issue_key = IssueKey::from_issue_ref(&reference);
    record_event_for_key(
        paths,
        issue_key.clone(),
        event_type,
        RecommendationEventSource::FeedbackCommand,
    )?;
    Ok(format!(
        "Recorded {event_type:?} feedback for {}",
        issue_key.label()
    ))
}

pub fn render_feedback_state(paths: &IssueFinderPaths, issue: &str) -> Result<String> {
    paths.ensure_layout()?;
    let reference = IssueRef::parse(issue)?;
    let issue_key = IssueKey::from_issue_ref(&reference);
    let states = load_state_map(paths)?;
    let state = states.get(&issue_key).cloned().unwrap_or_else(|| {
        crate::recommendation::RecommendationIssueState {
            issue_key: issue_key.clone(),
            ..Default::default()
        }
    });
    let events = recent_events_for_issue(paths, &issue_key, 10)?;
    let mut lines = vec![
        format!("Feedback state for {}", issue_key.label()),
        format!("- shown: {}", state.shown_count),
        format!("- read: {}", state.read_count),
        format!("- prepared: {}", state.prepared_count),
        format!("- done: {}", state.done),
        format!("- dismissed: {}", state.dismissed),
        format!(
            "- last feedback: {}",
            state.last_feedback_at.as_deref().unwrap_or("none")
        ),
        format!(
            "- last seen issue updated: {}",
            state
                .last_seen_issue_updated_at
                .as_deref()
                .unwrap_or("none")
        ),
        format!(
            "- last seen comments: {}",
            state
                .last_seen_comments_count
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        "Recent events:".to_string(),
    ];
    if events.is_empty() {
        lines.push("- none".to_string());
    } else {
        lines.extend(events.into_iter().map(|event| {
            format!(
                "- {} | {:?} | {:?}",
                event.timestamp, event.event_type, event.source
            )
        }));
    }
    Ok(lines.join("\n"))
}

pub fn render_ranked(ranked: &[RankedValueIssue]) -> String {
    if ranked.is_empty() {
        return "No issues found".to_string();
    }

    ranked
        .iter()
        .enumerate()
        .map(|(index, issue)| {
            let risk_tags = if issue.value_assessment.risk_tags.is_empty() {
                "none".to_string()
            } else {
                issue
                    .value_assessment
                    .risk_tags
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let detail = format!(
                "{} | rank {} | feed {} | freshness +{} | feedback -{} | quality -{} | reactivation +{} | visibility {} | repo {}:{} | competition {}:{} | profile {}:{} | execution {} ({}) | fit {} | risk {} | evidence: {} | risks: {}",
                issue.value_assessment.recommendation_category,
                issue.value_assessment.final_rank_score,
                issue.recommendation.final_feed_score,
                issue.recommendation.freshness_boost,
                issue.recommendation.feedback_penalty,
                issue.recommendation.quality_penalty,
                issue.recommendation.reactivation_boost,
                issue.recommendation.visibility,
                issue.value_assessment.gates.repo_influence.status,
                issue.value_assessment.gates.repo_influence.band,
                issue.value_assessment.gates.competition.status,
                issue.value_assessment.gates.competition.band,
                issue.value_assessment.gates.profile_fit.status,
                issue.value_assessment.gates.profile_fit.band,
                issue.value_assessment.execution_score,
                issue.value_assessment.execution_band,
                issue.value_assessment.profile_fit_score,
                issue.value_assessment.risk_penalty,
                issue.explanation.join("; "),
                risk_tags
            );
            format!(
                "{}. {}#{} | feed {} | rank {} | {}\n   {}\n   {}",
                index + 1,
                issue.issue.repo_full_name,
                issue.issue.number,
                issue.recommendation.final_feed_score,
                issue.value_assessment.final_rank_score,
                issue.issue.title,
                issue.issue.url,
                detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_prepare_outcome(outcome: &PrepareOutcome) -> String {
    match outcome {
        PrepareOutcome::Prepared(item) => format!(
            "Prepared {}\nCategory: {} | attention {} | execution {} | risk {}\nJSON: {}\nMarkdown: {}\nCodex: {}",
            item.id,
            item.recommendation_category,
            item.attention_score,
            item.execution_score,
            item.risk_penalty,
            item.handoff_json_path,
            item.handoff_md_path,
            item.codex_md_path
        ),
        PrepareOutcome::Failed(item) => format!(
            "Preparation failed for {}#{}\nReason: {}",
            item.repo_full_name, item.issue_number, item.reason
        ),
    }
}

pub fn render_daily(report: &DailyReport, path: &str) -> String {
    format!(
        "Daily report written: {path}\nPrepared: {}\nFailed: {}",
        report.prepared.len(),
        report.failed.len()
    )
}

fn prepared_report_item(
    ranked: &RankedValueIssue,
    evidence_pack: &EvidencePack,
    handoff: &Handoff,
    written: &WrittenHandoff,
) -> PreparedReportItem {
    PreparedReportItem {
        id: written.id.clone(),
        repo_full_name: ranked.issue.repo_full_name.clone(),
        issue_number: ranked.issue.number,
        title: ranked.issue.title.clone(),
        score: ranked.score,
        final_rank_score: ranked.value_assessment.final_rank_score,
        feed_score: ranked.recommendation.final_feed_score,
        freshness_boost: ranked.recommendation.freshness_boost,
        feedback_penalty: ranked.recommendation.feedback_penalty,
        quality_penalty: ranked.recommendation.quality_penalty,
        reactivation_boost: ranked.recommendation.reactivation_boost,
        recommendation_visibility: ranked.recommendation.visibility.to_string(),
        recommendation_reasons: ranked.recommendation.reasons.clone(),
        attention_score: ranked.value_assessment.attention_score,
        execution_score: ranked.value_assessment.execution_score,
        profile_fit_score: ranked.value_assessment.profile_fit_score,
        risk_penalty: ranked.value_assessment.risk_penalty,
        recommendation_category: ranked.value_assessment.recommendation_category.to_string(),
        risk_tags: ranked
            .value_assessment
            .risk_tags
            .iter()
            .map(ToString::to_string)
            .collect(),
        why_it_is_worth_doing: evidence_pack
            .why_this_has_high_attention
            .first()
            .map(|item| item.summary.clone())
            .unwrap_or_else(|| "Attention evidence is limited".to_string()),
        biggest_risk: evidence_pack
            .risk_factors
            .first()
            .map(|item| item.summary.clone())
            .unwrap_or_else(|| "none".to_string()),
        missing_evidence: evidence_pack.missing_evidence.clone(),
        handoff_json_path: written.handoff_json_path.clone(),
        handoff_md_path: written.handoff_md_path.clone(),
        codex_md_path: written.codex_md_path.clone(),
        agent_policy_path: written.agent_policy_path.clone(),
        probe_json_path: written.probe_json_path.clone(),
        prepare_events_path: written.prepare_events_path.clone(),
        readiness_score: handoff.readiness.score,
        readiness_band: handoff.readiness.band.clone(),
        probe_status: handoff.probe_pack.status.clone(),
        probe_warnings: handoff.probe_pack.warnings.clone(),
    }
}
