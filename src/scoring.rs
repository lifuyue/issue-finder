use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::ProfileConfig;
use crate::github::GitHubIssue;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedIssue {
    pub issue: GitHubIssue,
    pub score: i32,
    pub explanation: Vec<String>,
}

pub fn rank_issues(issues: Vec<GitHubIssue>, profile: &ProfileConfig) -> Vec<RankedIssue> {
    let mut ranked = issues
        .into_iter()
        .map(|issue| score_issue(issue, profile))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.issue.updated_at.cmp(&left.issue.updated_at))
    });
    ranked
}

pub fn score_issue(issue: GitHubIssue, profile: &ProfileConfig) -> RankedIssue {
    let terms = profile_terms(profile);
    let title = normalize(&issue.title);
    let body = normalize(&issue.body);
    let repo = normalize(&format!(
        "{} {}",
        issue.repo_full_name, issue.repo_description
    ));
    let labels = normalize(&issue.labels.join(" "));

    let mut score = 0;
    let mut explanation = Vec::new();

    for term in terms {
        let mut term_score = 0;
        if title.contains(&term) {
            term_score += 18;
        }
        if body.contains(&term) {
            term_score += 5;
        }
        if repo.contains(&term) {
            term_score += 8;
        }
        if labels.contains(&term) {
            term_score += 12;
        }
        if term_score > 0 {
            score += term_score;
            explanation.push(format!("matched profile term `{term}` (+{term_score})"));
        }
    }

    if labels.contains("good first issue") {
        score += 15;
        explanation.push("has good-first-issue label (+15)".to_string());
    }

    if has_actionable_signal(&issue.title, &issue.body) {
        score += 12;
        explanation.push("issue includes actionable signals (+12)".to_string());
    }

    if issue.body.trim().len() >= 120 {
        score += 6;
        explanation.push("issue body has useful detail (+6)".to_string());
    }

    let freshness = freshness_boost(&issue.updated_at);
    if freshness > 0 {
        score += freshness;
        explanation.push(format!("recently updated (+{freshness})"));
    }

    let star_boost = ((issue.repo_stars + 10) as f64).log10() * 5.0;
    let star_boost = star_boost.round().min(12.0) as i32;
    if star_boost > 0 {
        score += star_boost;
        explanation.push(format!("repository stars (+{star_boost})"));
    }

    score = score.clamp(0, 100);
    if explanation.is_empty() {
        explanation.push("no strong local match signals".to_string());
    }

    RankedIssue {
        issue,
        score,
        explanation,
    }
}

pub fn profile_terms(profile: &ProfileConfig) -> Vec<String> {
    let mut terms = HashSet::new();
    for item in profile.tech_stack.iter().chain(profile.keywords.iter()) {
        let normalized = normalize(item);
        if normalized.len() >= 2 {
            terms.insert(normalized.clone());
        }
        for alias in aliases(&normalized) {
            terms.insert(alias.to_string());
        }
    }
    let mut values = terms.into_iter().collect::<Vec<_>>();
    values.sort();
    values
}

pub fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .replace("++", " plus plus")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn has_actionable_signal(title: &str, body: &str) -> bool {
    let text = format!("{title}\n{body}").to_lowercase();
    let action_words = [
        "steps to reproduce",
        "step to reproduce",
        "expected",
        "actual",
        "acceptance criteria",
        "stack trace",
        "repro",
    ];
    if action_words.iter().any(|word| text.contains(word)) {
        return true;
    }

    text.split_whitespace().any(|token| {
        let token = token.trim_matches(|ch: char| {
            !ch.is_ascii_alphanumeric() && ch != '/' && ch != '.' && ch != '-' && ch != '_'
        });
        token.contains('/')
            && [
                ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".md", ".json",
            ]
            .iter()
            .any(|suffix| token.ends_with(suffix))
    })
}

fn freshness_boost(updated_at: &str) -> i32 {
    let Ok(updated_at) = DateTime::parse_from_rfc3339(updated_at) else {
        return 0;
    };
    let age_hours = (Utc::now() - updated_at.with_timezone(&Utc)).num_hours();
    match age_hours {
        value if value <= 24 => 12,
        value if value <= 72 => 10,
        value if value <= 24 * 7 => 7,
        value if value <= 24 * 14 => 4,
        _ => 0,
    }
}

fn aliases(term: &str) -> &'static [&'static str] {
    match term {
        "typescript" => &["ts", "tsx"],
        "javascript" => &["js", "jsx"],
        "node js" => &["node", "nodejs", "npm"],
        "react" => &["jsx", "tsx", "component", "hooks"],
        "python" => &["py", "pytest"],
        "go" => &["golang"],
        "rust" => &["cargo", "rs"],
        "cli" => &["command line"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{rank_issues, score_issue};
    use crate::config::ProfileConfig;
    use crate::github::GitHubIssue;

    fn issue(title: &str, body: &str, labels: Vec<&str>, stars: u64) -> GitHubIssue {
        GitHubIssue {
            id: 1,
            number: 1,
            title: title.to_string(),
            body: body.to_string(),
            labels: labels.into_iter().map(str::to_string).collect(),
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: "Rust CLI developer tools".to_string(),
            repo_stars: stars,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn scores_profile_and_actionable_signals() {
        let profile = ProfileConfig {
            tech_stack: vec!["Rust".to_string()],
            keywords: vec!["cli".to_string()],
        };
        let ranked = score_issue(
            issue(
                "Fix Rust CLI panic",
                "Expected no panic. Actual panic in src/main.rs",
                vec!["good first issue"],
                100,
            ),
            &profile,
        );
        assert!(ranked.score >= 50);
        assert!(ranked
            .explanation
            .iter()
            .any(|item| item.contains("actionable")));
    }

    #[test]
    fn ranks_higher_scoring_issue_first() {
        let profile = ProfileConfig {
            tech_stack: vec!["Rust".to_string()],
            keywords: vec!["cli".to_string()],
        };
        let ranked = rank_issues(
            vec![
                issue("Update docs", "", vec!["good first issue"], 1),
                issue(
                    "Fix Rust CLI bug",
                    "Expected behavior in src/main.rs",
                    vec!["good first issue"],
                    10,
                ),
            ],
            &profile,
        );
        assert_eq!(ranked[0].issue.title, "Fix Rust CLI bug");
    }
}
