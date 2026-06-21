use anyhow::Result;
use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::{atomic_write, IssueFinderPaths};
use crate::value_scoring::RecommendationCategory;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DailyReport {
    pub run_timestamp: String,
    pub discovery_count: usize,
    pub prepared: Vec<PreparedReportItem>,
    pub failed: Vec<FailedReportItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreparedReportItem {
    pub id: String,
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub score: i32,
    pub final_rank_score: i32,
    #[serde(default)]
    pub feed_score: i32,
    #[serde(default)]
    pub freshness_boost: i32,
    #[serde(default)]
    pub feedback_penalty: i32,
    #[serde(default)]
    pub quality_penalty: i32,
    #[serde(default)]
    pub reactivation_boost: i32,
    #[serde(default)]
    pub memory_adjustment: i32,
    #[serde(default)]
    pub recommendation_visibility: String,
    #[serde(default)]
    pub recommendation_reasons: Vec<String>,
    pub attention_score: i32,
    pub execution_score: i32,
    pub profile_fit_score: i32,
    pub risk_penalty: i32,
    pub recommendation_category: String,
    pub risk_tags: Vec<String>,
    pub why_it_is_worth_doing: String,
    pub biggest_risk: String,
    pub missing_evidence: Vec<String>,
    pub handoff_json_path: String,
    pub handoff_md_path: String,
    #[serde(default)]
    pub codex_md_path: String,
    #[serde(default)]
    pub agent_policy_path: String,
    #[serde(default)]
    pub probe_json_path: String,
    #[serde(default)]
    pub prepare_events_path: String,
    #[serde(default)]
    pub readiness_score: i32,
    #[serde(default)]
    pub readiness_band: String,
    #[serde(default)]
    pub probe_status: String,
    #[serde(default)]
    pub probe_warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FailedReportItem {
    pub repo_full_name: String,
    pub issue_number: u64,
    pub title: String,
    pub score: i32,
    pub reason: String,
}

impl DailyReport {
    pub fn render_markdown(&self) -> String {
        let mut lines = vec![
            format!(
                "# Issue Finder Daily Report - {}",
                Local::now().format("%Y-%m-%d")
            ),
            String::new(),
            format!("- Run timestamp: {}", self.run_timestamp),
            format!("- Discovery count: {}", self.discovery_count),
            format!("- Prepared handoff count: {}", self.prepared.len()),
            format!("- Failed preparation count: {}", self.failed.len()),
            String::new(),
            "## Recommended Tasks".to_string(),
            String::new(),
        ];

        if self.prepared.is_empty() {
            lines.push("- No prepared tasks today".to_string());
        } else {
            for category in [
                RecommendationCategory::HighValueReady,
                RecommendationCategory::HighValueNeedsScoping,
                RecommendationCategory::NicheButActionable,
                RecommendationCategory::ContestedOrLowTrust,
                RecommendationCategory::NeedsTriage,
                RecommendationCategory::FilteredLowDepth,
            ] {
                push_category_group(&mut lines, category, &self.prepared);
            }
        }

        lines.extend([
            String::new(),
            "## Prepared Handoffs".to_string(),
            String::new(),
        ]);
        if self.prepared.is_empty() {
            lines.push("- None".to_string());
        } else {
            for item in &self.prepared {
                lines.push(format!(
                    "- [{}] {}#{} | feed {} | rank {} | freshness +{} | feedback -{} | quality -{} | reactivation +{} | memory {:+} | attention {} | execution {} | fit {} | risk {} | readiness {} ({}) | probe {} | probe warnings: {} | category {} | visibility {} | tags: {} | risk detail: {} | recommendation detail: {} | missing: {} | JSON: {} | Markdown: {} | Codex: {} | Policy: {} | Probe: {} | Events: {}",
                    item.id,
                    item.repo_full_name,
                    item.issue_number,
                    item.feed_score,
                    item.final_rank_score,
                    item.freshness_boost,
                    item.feedback_penalty,
                    item.quality_penalty,
                    item.reactivation_boost,
                    item.memory_adjustment,
                    item.attention_score,
                    item.execution_score,
                    item.profile_fit_score,
                    item.risk_penalty,
                    item.readiness_score,
                    if item.readiness_band.is_empty() {
                        "unknown"
                    } else {
                        &item.readiness_band
                    },
                    if item.probe_status.is_empty() {
                        "unknown"
                    } else {
                        &item.probe_status
                    },
                    if item.probe_warnings.is_empty() {
                        "none".to_string()
                    } else {
                        item.probe_warnings.join("; ")
                    },
                    item.recommendation_category,
                    if item.recommendation_visibility.is_empty() {
                        "unknown"
                    } else {
                        &item.recommendation_visibility
                    },
                    if item.risk_tags.is_empty() {
                        "none".to_string()
                    } else {
                        item.risk_tags.join(", ")
                    },
                    item.biggest_risk,
                    if item.recommendation_reasons.is_empty() {
                        "none".to_string()
                    } else {
                        item.recommendation_reasons.join("; ")
                    },
                    if item.missing_evidence.is_empty() {
                        "none".to_string()
                    } else {
                        item.missing_evidence.join("; ")
                    },
                    item.handoff_json_path,
                    item.handoff_md_path,
                    item.codex_md_path,
                    item.agent_policy_path,
                    item.probe_json_path,
                    item.prepare_events_path
                ));
            }
        }

        lines.extend([
            String::new(),
            "## Failed Preparations".to_string(),
            String::new(),
        ]);
        if self.failed.is_empty() {
            lines.push("- None".to_string());
        } else {
            for item in &self.failed {
                lines.push(format!(
                    "- {}#{} | score {} | {} | reason: {}",
                    item.repo_full_name, item.issue_number, item.score, item.title, item.reason
                ));
            }
        }

        lines.push(String::new());
        lines.join("\n")
    }
}

fn push_category_group(
    lines: &mut Vec<String>,
    category: RecommendationCategory,
    prepared: &[PreparedReportItem],
) {
    lines.push(format!("### {}", category_heading(category)));
    lines.push(String::new());
    let mut matched = prepared
        .iter()
        .filter(|item| item.recommendation_category == category.to_string())
        .peekable();
    if matched.peek().is_none() {
        lines.push("- None".to_string());
    } else {
        for item in matched {
            lines.push(format!(
                "- {}#{} | feed {} | rank {} | attention {} | execution {} | readiness {} ({}) | risk {} | {}",
                item.repo_full_name,
                item.issue_number,
                item.feed_score,
                item.final_rank_score,
                item.attention_score,
                item.execution_score,
                item.readiness_score,
                if item.readiness_band.is_empty() {
                    "unknown"
                } else {
                    &item.readiness_band
                },
                item.risk_penalty,
                item.why_it_is_worth_doing
            ));
        }
    }
    lines.push(String::new());
}

fn category_heading(category: RecommendationCategory) -> &'static str {
    match category {
        RecommendationCategory::HighValueReady => "High-value Ready",
        RecommendationCategory::HighValueNeedsScoping => "High-value Needs Scoping",
        RecommendationCategory::NicheButActionable => "Niche but Actionable",
        RecommendationCategory::ContestedOrLowTrust => "Contested or Low Trust",
        RecommendationCategory::FilteredLowDepth => "Filtered Low Depth",
        RecommendationCategory::NeedsTriage => "Needs Triage",
    }
}

pub fn write_daily_report(paths: &IssueFinderPaths, report: &DailyReport) -> Result<String> {
    let date = Local::now().format("%Y-%m-%d").to_string();
    let path = paths.report_path(&date);
    atomic_write(&path, report.render_markdown())?;
    Ok(path.to_string_lossy().to_string())
}

pub fn empty_report(discovery_count: usize) -> DailyReport {
    DailyReport {
        run_timestamp: Utc::now().to_rfc3339(),
        discovery_count,
        prepared: Vec::new(),
        failed: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{DailyReport, PreparedReportItem};

    #[test]
    fn renders_report_sections() {
        let report = DailyReport {
            run_timestamp: Utc::now().to_rfc3339(),
            discovery_count: 10,
            prepared: vec![PreparedReportItem {
                id: "id".to_string(),
                repo_full_name: "owner/repo".to_string(),
                issue_number: 1,
                title: "Issue".to_string(),
                score: 90,
                final_rank_score: 90,
                feed_score: 590,
                freshness_boost: 10,
                feedback_penalty: 0,
                quality_penalty: 0,
                reactivation_boost: 0,
                memory_adjustment: 0,
                recommendation_visibility: "visible".to_string(),
                recommendation_reasons: Vec::new(),
                attention_score: 80,
                execution_score: 85,
                profile_fit_score: 50,
                risk_penalty: 5,
                recommendation_category: "high_value_ready".to_string(),
                risk_tags: Vec::new(),
                why_it_is_worth_doing: "High value evidence".to_string(),
                biggest_risk: "none".to_string(),
                missing_evidence: Vec::new(),
                handoff_json_path: "/tmp/handoff.json".to_string(),
                handoff_md_path: "/tmp/handoff.md".to_string(),
                codex_md_path: "/tmp/codex.md".to_string(),
                agent_policy_path: "/tmp/agent-policy.json".to_string(),
                probe_json_path: "/tmp/probe.json".to_string(),
                prepare_events_path: "/tmp/prepare-events.jsonl".to_string(),
                readiness_score: 82,
                readiness_band: "high".to_string(),
                probe_status: "completed".to_string(),
                probe_warnings: Vec::new(),
            }],
            failed: Vec::new(),
        };

        let markdown = report.render_markdown();
        assert!(markdown.contains("Prepared handoff count: 1"));
        assert!(markdown.contains("High-value Ready"));
    }
}
