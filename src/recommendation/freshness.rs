use chrono::{DateTime, Utc};

use crate::github_enrichment::EnrichedIssue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshnessAssessment {
    pub boost: i32,
    pub reasons: Vec<String>,
}

pub fn assess_freshness(enriched: &EnrichedIssue) -> FreshnessAssessment {
    let mut boost = issue_updated_boost(&enriched.issue.updated_at);
    let mut reasons = Vec::new();

    if boost > 0 {
        reasons.push(format!("Issue was recently updated (+{boost})"));
    }
    if enriched.activity.maintainer_recent_response {
        boost += 20;
        reasons.push("Maintainer recently responded (+20)".to_string());
    }
    if enriched.activity.recent_repo_activity {
        boost += 8;
        reasons.push("Repository was recently active (+8)".to_string());
    }
    if enriched.activity.recent_issue_activity {
        boost += 8;
        reasons.push("Issue has recent activity (+8)".to_string());
    }

    FreshnessAssessment { boost, reasons }
}

fn issue_updated_boost(updated_at: &str) -> i32 {
    let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at) else {
        return 0;
    };
    let age_hours = (Utc::now() - updated_at.with_timezone(&Utc)).num_hours();
    match age_hours {
        value if value <= 24 => 45,
        value if value <= 24 * 3 => 36,
        value if value <= 24 * 7 => 28,
        value if value <= 24 * 14 => 18,
        value if value <= 24 * 30 => 10,
        _ => 0,
    }
}
