use std::fs;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::github::{GitHubClient, GitHubIssue, IssueRef};
use crate::handoff::{write_handoff, Handoff};
use crate::inbox;
use crate::llm;
use crate::paths::PatchbayPaths;
use crate::report::{self, DailyReport, FailedReportItem, PreparedReportItem};
use crate::scoring::{rank_issues, score_issue, RankedIssue};
use crate::workspace;

#[derive(Debug, Clone)]
pub enum PrepareOutcome {
    Prepared(PreparedReportItem),
    Failed(FailedReportItem),
}

pub async fn scout(
    paths: &PatchbayPaths,
    config: &Config,
    limit: usize,
    refresh: bool,
) -> Result<Vec<RankedIssue>> {
    paths.ensure_layout()?;
    let github = GitHubClient::new(config)?;
    let issues = github.discover_issues(paths, refresh).await?;
    let mut ranked = rank_issues(issues, &config.profile);
    ranked.truncate(limit);
    Ok(ranked)
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
    prepare_issue(paths, config, issue).await
}

pub async fn prepare_issue(
    paths: &PatchbayPaths,
    config: &Config,
    issue: GitHubIssue,
) -> Result<PrepareOutcome> {
    let ranked = score_issue(issue.clone(), &config.profile);
    match workspace::prepare_workspace(paths, &issue) {
        Ok(workspace) => {
            let mut handoff = Handoff::build(&issue, &workspace);
            llm::enhance_handoff(config, &mut handoff).await;
            let written = write_handoff(paths, &handoff, &issue)?;
            inbox::upsert_ready(paths, &issue, ranked.score, &written)?;
            Ok(PrepareOutcome::Prepared(PreparedReportItem {
                id: written.id,
                repo_full_name: issue.repo_full_name,
                issue_number: issue.number,
                title: issue.title,
                score: ranked.score,
                handoff_json_path: written.handoff_json_path,
                handoff_md_path: written.handoff_md_path,
            }))
        }
        Err(error) => {
            let reason = error.to_string();
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
    let issues = github.discover_issues(paths, refresh).await?;
    let discovery_count = issues.len();
    let ranked = rank_issues(issues, &config.profile);

    daily_from_ranked(paths, config, ranked, discovery_count, top_n).await
}

pub async fn daily_from_ranked(
    paths: &PatchbayPaths,
    config: &Config,
    ranked: Vec<RankedIssue>,
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

        attempts += 1;
        let issue = ranked_issue.issue;
        match prepare_issue(paths, config, issue.clone()).await {
            Ok(PrepareOutcome::Prepared(item)) => report.prepared.push(item),
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

pub fn render_ranked(ranked: &[RankedIssue]) -> String {
    if ranked.is_empty() {
        return "No issues found".to_string();
    }

    ranked
        .iter()
        .enumerate()
        .map(|(index, issue)| {
            format!(
                "{}. {}#{} | score {} | {}\n   {}\n   {}",
                index + 1,
                issue.issue.repo_full_name,
                issue.issue.number,
                issue.score,
                issue.issue.title,
                issue.issue.url,
                issue.explanation.join("; ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_prepare_outcome(outcome: &PrepareOutcome) -> String {
    match outcome {
        PrepareOutcome::Prepared(item) => format!(
            "Prepared {}\nJSON: {}\nMarkdown: {}",
            item.id, item.handoff_json_path, item.handoff_md_path
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
