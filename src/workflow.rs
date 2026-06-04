use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::config::Config;
use crate::evidence_pack::{build_evidence_pack, EvidencePack};
use crate::github::{GitHubClient, GitHubIssue, IssueRef};
use crate::github_enrichment::GitHubEnrichmentClient;
use crate::handoff::{handoff_id, write_handoff_with_events, Handoff, WrittenHandoff};
use crate::inbox;
use crate::llm;
use crate::llm_review;
use crate::paths::PatchbayPaths;
use crate::prepare_events::PrepareEventLog;
use crate::probe::SafeProbeRunner;
use crate::readiness::assess_readiness;
use crate::report::{self, DailyReport, FailedReportItem, PreparedReportItem};
use crate::scoring::rank_issues;
use crate::value_scoring::{
    assess_issue, is_daily_prepare_candidate, RankedValueIssue, ValueAssessment,
};
use crate::workspace;

const ENRICHED_SCOUT_CANDIDATE_LIMIT: usize = 40;
const COMPETITION_TIMELINE_CANDIDATE_LIMIT: usize = 20;

#[derive(Debug, Clone)]
pub enum PrepareOutcome {
    Prepared(Box<PreparedReportItem>),
    Failed(FailedReportItem),
}

#[derive(Debug, Clone, Default)]
pub struct PrepareOptions {
    pub explicit_prepare: bool,
    pub gate_bypass_reason: Option<String>,
}

pub async fn scout(
    paths: &PatchbayPaths,
    config: &Config,
    limit: usize,
    refresh: bool,
) -> Result<Vec<RankedValueIssue>> {
    paths.ensure_layout()?;
    let github = GitHubClient::new(config)?;
    let enrichment = GitHubEnrichmentClient::new(config)?;
    let issues = github.discover_issues(paths, refresh).await?;
    let mut ranked = rank_issues(issues, &config.profile);
    ranked.truncate(limit.clamp(25, ENRICHED_SCOUT_CANDIDATE_LIMIT));
    let mut ranked = enrich_ranked_issues(paths, config, &enrichment, ranked, refresh).await;
    sort_by_value(&mut ranked);
    ranked.truncate(limit);
    Ok(ranked)
}

pub async fn assess_from_input(
    paths: &PatchbayPaths,
    config: &Config,
    issue: Option<String>,
    url: Option<String>,
    refresh: bool,
) -> Result<RankedValueIssue> {
    paths.ensure_layout()?;
    let reference = match (issue, url) {
        (Some(issue), None) => IssueRef::parse(&issue)?,
        (None, Some(url)) => IssueRef::parse_url(&url)?,
        (Some(_), Some(_)) => anyhow::bail!("pass either an issue reference or --url, not both"),
        (None, None) => {
            anyhow::bail!("pass owner/repo#123 or --url https://github.com/owner/repo/issues/123")
        }
    };

    let github = GitHubClient::new(config)?;
    let issue = github.fetch_issue(&reference).await?;
    let enrichment = GitHubEnrichmentClient::new(config)?;
    Ok(enrich_issue_for_value(paths, config, &enrichment, issue, refresh).await)
}

pub async fn prepare_from_input(
    paths: &PatchbayPaths,
    config: &Config,
    issue: Option<String>,
    url: Option<String>,
) -> Result<PrepareOutcome> {
    paths.ensure_layout()?;
    let reference = match (issue, url) {
        (Some(issue), None) => IssueRef::parse(&issue)?,
        (None, Some(url)) => IssueRef::parse_url(&url)?,
        (Some(_), Some(_)) => anyhow::bail!("pass either an issue reference or --url, not both"),
        (None, None) => {
            anyhow::bail!("pass owner/repo#123 or --url https://github.com/owner/repo/issues/123")
        }
    };

    let github = GitHubClient::new(config)?;
    let issue = github.fetch_issue(&reference).await?;
    let enrichment = GitHubEnrichmentClient::new(config)?;
    let ranked = enrich_issue_for_value(paths, config, &enrichment, issue, false).await;
    prepare_value_issue(paths, config, ranked, true).await
}

pub async fn prepare_issue(
    paths: &PatchbayPaths,
    config: &Config,
    issue: GitHubIssue,
) -> Result<PrepareOutcome> {
    let enrichment = GitHubEnrichmentClient::new(config)?;
    let ranked = enrich_issue_for_value(paths, config, &enrichment, issue, false).await;
    prepare_value_issue(paths, config, ranked, true).await
}

pub async fn prepare_value_issue(
    paths: &PatchbayPaths,
    config: &Config,
    ranked: RankedValueIssue,
    explicit_prepare: bool,
) -> Result<PrepareOutcome> {
    prepare_value_issue_with_options(
        paths,
        config,
        ranked,
        PrepareOptions {
            explicit_prepare,
            gate_bypass_reason: None,
        },
    )
    .await
}

pub async fn prepare_value_issue_with_options(
    paths: &PatchbayPaths,
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
            let mut handoff = Handoff::build_with_value(
                &issue,
                &workspace,
                ranked.value_assessment.clone(),
                evidence_pack.clone(),
                llm_review,
            );
            handoff.probe_pack = probe_pack;
            handoff.readiness = readiness;
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
    paths: &PatchbayPaths,
    config: &Config,
    top: Option<usize>,
    refresh: bool,
) -> Result<(DailyReport, String)> {
    paths.ensure_layout()?;
    let top_n = top.unwrap_or(config.daily.top_n).max(1);
    let github = GitHubClient::new(config)?;
    let enrichment = GitHubEnrichmentClient::new(config)?;
    let issues = github.discover_issues(paths, refresh).await?;
    let discovery_count = issues.len();
    let ranked = rank_issues(issues, &config.profile);
    let mut ranked = enrich_ranked_issues(
        paths,
        config,
        &enrichment,
        ranked
            .into_iter()
            .take(ENRICHED_SCOUT_CANDIDATE_LIMIT)
            .collect(),
        refresh,
    )
    .await;
    sort_by_value(&mut ranked);

    daily_from_ranked(paths, config, ranked, discovery_count, top_n).await
}

pub async fn daily_from_ranked(
    paths: &PatchbayPaths,
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
        if inbox::contains_issue(
            paths,
            &ranked_issue.issue.repo_full_name,
            ranked_issue.issue.number,
        )? {
            continue;
        }
        if !is_daily_prepare_candidate(&ranked_issue.value_assessment) {
            continue;
        }

        attempts += 1;
        let issue = ranked_issue.issue.clone();
        match prepare_value_issue(paths, config, ranked_issue.clone(), false).await {
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

pub fn read_handoff(paths: &PatchbayPaths, id: &str, json: bool) -> Result<String> {
    let item = inbox::find_item(paths, id)?;
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

pub fn read_report(paths: &PatchbayPaths, date: Option<String>) -> Result<String> {
    let date = date.unwrap_or_else(|| chrono::Local::now().format("%Y-%m-%d").to_string());
    let path = paths.report_path(&date);
    fs::read_to_string(&path).with_context(|| format!("unable to read {}", path.display()))
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
                "{} | rank {} | repo {}:{} | competition {}:{} | profile {}:{} | execution {} ({}) | fit {} | risk {} | evidence: {} | risks: {}",
                issue.value_assessment.recommendation_category,
                issue.value_assessment.final_rank_score,
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
                "{}. {}#{} | rank {} | {}\n   {}\n   {}",
                index + 1,
                issue.issue.repo_full_name,
                issue.issue.number,
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

async fn enrich_ranked_issues(
    paths: &PatchbayPaths,
    config: &Config,
    enrichment: &GitHubEnrichmentClient,
    ranked: Vec<crate::scoring::RankedIssue>,
    refresh: bool,
) -> Vec<RankedValueIssue> {
    let mut values = Vec::new();
    for (index, rough) in ranked.into_iter().enumerate() {
        values.push(
            enrich_issue_for_value_with_competition(
                paths,
                config,
                enrichment,
                rough.issue,
                refresh,
                index < COMPETITION_TIMELINE_CANDIDATE_LIMIT,
            )
            .await,
        );
    }
    values
}

async fn enrich_issue_for_value(
    paths: &PatchbayPaths,
    config: &Config,
    enrichment: &GitHubEnrichmentClient,
    issue: GitHubIssue,
    refresh: bool,
) -> RankedValueIssue {
    enrich_issue_for_value_with_competition(paths, config, enrichment, issue, refresh, true).await
}

async fn enrich_issue_for_value_with_competition(
    paths: &PatchbayPaths,
    config: &Config,
    enrichment: &GitHubEnrichmentClient,
    issue: GitHubIssue,
    refresh: bool,
    include_competition_timeline: bool,
) -> RankedValueIssue {
    let enriched = enrichment
        .enrich_issue_with_options(paths, &issue, refresh, include_competition_timeline)
        .await;
    let value_assessment = assess_issue(&enriched, &config.profile);
    ranked_value_issue(issue, value_assessment, enriched)
}

fn ranked_value_issue(
    issue: GitHubIssue,
    value_assessment: ValueAssessment,
    enriched_issue: crate::github_enrichment::EnrichedIssue,
) -> RankedValueIssue {
    let score = value_assessment.final_rank_score;
    let explanation = value_assessment.explanation.clone();
    RankedValueIssue {
        issue,
        score,
        value_assessment,
        enriched_issue,
        explanation,
    }
}

fn sort_by_value(ranked: &mut [RankedValueIssue]) {
    ranked.sort_by(|left, right| {
        left.value_assessment
            .recommendation_category
            .sort_rank()
            .cmp(&right.value_assessment.recommendation_category.sort_rank())
            .then_with(|| {
                right
                    .value_assessment
                    .final_rank_score
                    .cmp(&left.value_assessment.final_rank_score)
            })
            .then_with(|| {
                right
                    .value_assessment
                    .execution_score
                    .cmp(&left.value_assessment.execution_score)
            })
            .then_with(|| {
                right
                    .value_assessment
                    .profile_fit_score
                    .cmp(&left.value_assessment.profile_fit_score)
            })
            .then_with(|| {
                right
                    .value_assessment
                    .attention_score
                    .cmp(&left.value_assessment.attention_score)
            })
    });
}

#[allow(dead_code)]
fn legacy_sort_by_score(ranked: &mut [RankedValueIssue]) {
    ranked.sort_by(|left, right| {
        right
            .value_assessment
            .final_rank_score
            .cmp(&left.value_assessment.final_rank_score)
            .then_with(|| {
                right
                    .value_assessment
                    .attention_score
                    .cmp(&left.value_assessment.attention_score)
            })
            .then_with(|| {
                right
                    .value_assessment
                    .execution_score
                    .cmp(&left.value_assessment.execution_score)
            })
    });
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
