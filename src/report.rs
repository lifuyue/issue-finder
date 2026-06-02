use anyhow::Result;
use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};

use crate::paths::{atomic_write, PatchbayPaths};

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
    pub handoff_json_path: String,
    pub handoff_md_path: String,
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
                "# Patchbay Daily Report - {}",
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
            for item in self.prepared.iter().take(3) {
                lines.push(format!(
                    "- {}#{} | score {} | {}",
                    item.repo_full_name, item.issue_number, item.score, item.title
                ));
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
                    "- [{}] {}#{} | score {} | JSON: {} | Markdown: {}",
                    item.id,
                    item.repo_full_name,
                    item.issue_number,
                    item.score,
                    item.handoff_json_path,
                    item.handoff_md_path
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

pub fn write_daily_report(paths: &PatchbayPaths, report: &DailyReport) -> Result<String> {
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
                handoff_json_path: "/tmp/handoff.json".to_string(),
                handoff_md_path: "/tmp/handoff.md".to_string(),
            }],
            failed: Vec::new(),
        };

        let markdown = report.render_markdown();
        assert!(markdown.contains("Prepared handoff count: 1"));
        assert!(markdown.contains("Recommended Tasks"));
    }
}
