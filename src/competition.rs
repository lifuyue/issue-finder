use serde::{Deserialize, Serialize};
use std::fmt;

use crate::scoring::normalize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CompetitionFacts {
    #[serde(default)]
    pub open_pr_refs: usize,
    #[serde(default)]
    pub closed_pr_refs: usize,
    #[serde(default)]
    pub attempt_comments: usize,
    #[serde(default)]
    pub claim_comments: usize,
    #[serde(default)]
    pub working_comments: usize,
    #[serde(default)]
    pub fix_submitted_comments: usize,
    #[serde(default)]
    pub latest_competition_at: Option<String>,
    #[serde(default)]
    pub competition_points: i32,
    #[serde(default)]
    pub competition_band: CompetitionBand,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl CompetitionFacts {
    pub fn missing_timeline() -> Self {
        Self {
            warnings: vec!["Competition timeline evidence was not fetched".to_string()],
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompetitionBand {
    #[default]
    Clear,
    Light,
    Contested,
    Saturated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimelineIssueReference {
    pub source_ref: String,
    pub state: Option<String>,
    pub is_pull_request: bool,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommentCompetitionMarkers {
    pub attempt: bool,
    pub claim: bool,
    pub working: bool,
    pub fix_submitted: bool,
}

pub fn assess_competition(
    timeline_refs: &[TimelineIssueReference],
    comment_bodies: &[String],
    warnings: Vec<String>,
) -> CompetitionFacts {
    let open_pr_refs = timeline_refs
        .iter()
        .filter(|item| item.is_pull_request && is_open(item.state.as_deref()))
        .count();
    let closed_pr_refs = timeline_refs
        .iter()
        .filter(|item| item.is_pull_request && !is_open(item.state.as_deref()))
        .count();

    let mut attempt_comments = 0usize;
    let mut claim_comments = 0usize;
    let mut working_comments = 0usize;
    let mut fix_submitted_comments = 0usize;
    for body in comment_bodies {
        let markers = detect_comment_competition_markers(body);
        attempt_comments += usize::from(markers.attempt);
        claim_comments += usize::from(markers.claim);
        working_comments += usize::from(markers.working);
        fix_submitted_comments += usize::from(markers.fix_submitted);
    }

    let competition_points = (open_pr_refs * 3
        + closed_pr_refs
        + attempt_comments
        + claim_comments
        + working_comments
        + fix_submitted_comments) as i32;

    CompetitionFacts {
        open_pr_refs,
        closed_pr_refs,
        attempt_comments,
        claim_comments,
        working_comments,
        fix_submitted_comments,
        latest_competition_at: timeline_refs
            .iter()
            .filter_map(|item| item.created_at.clone())
            .max(),
        competition_points,
        competition_band: competition_band(competition_points),
        warnings,
    }
}

pub fn detect_comment_competition_markers(body: &str) -> CommentCompetitionMarkers {
    if is_issue_finder_projection_comment(body) {
        return CommentCompetitionMarkers::default();
    }

    let normalized = normalize(body);
    CommentCompetitionMarkers {
        attempt: body.contains("/attempt") || has_phrase(&normalized, "attempt"),
        claim: body.contains("/claim")
            || body.contains("/assign")
            || has_phrase(&normalized, "claim")
            || normalized.contains("assign this to me")
            || normalized.contains("assign to me")
            || normalized.contains("assign it to me")
            || normalized.contains("assign me")
            || normalized.contains(" claim ")
            || normalized.contains("ready to contribute")
            || normalized.contains("ready for contribute")
            || normalized.contains("can i contribute")
            || normalized.contains("could i contribute")
            || normalized.contains("can i work on this")
            || normalized.contains("could i work on this")
            || normalized.contains("interested in contributing")
            || normalized.contains("interested in working on this")
            || normalized.contains("external contributions be welcome")
            || normalized.contains("happy to implement")
            || normalized.contains("happy to work on this")
            || normalized.contains("i am ready to contribute")
            || normalized.contains("i d like to work on this")
            || normalized.contains("i would like to work on this")
            || normalized.contains("i d like to work on it")
            || normalized.contains("i would like to work on it")
            || normalized.contains("i would love to work on this")
            || normalized.contains("would like to work on this")
            || normalized.contains("would like to work on it")
            || normalized.contains("would love to work on this")
            || normalized.contains("i d like to take a look")
            || normalized.contains("i would like to take a look")
            || normalized.contains("i d like to fix this")
            || normalized.contains("i would like to fix this")
            || normalized.contains("i d like to take this issue")
            || normalized.contains("i would like to take this issue")
            || normalized.contains("i would like to be assigned")
            || normalized.contains("would like to be assigned")
            || normalized.contains("could i be assigned")
            || normalized.contains("can i get this issue")
            || normalized.contains("i want to work on this")
            || normalized.contains("i want to work on it")
            || normalized.contains("i want to take this")
            || normalized.contains("i want to pick this")
            || normalized.contains("want to pick this")
            || normalized.contains("can i work on it")
            || normalized.contains("could i work on it")
            || normalized.contains("take it up")
            || normalized.contains("take this issue up")
            || normalized.contains("can take this on")
            || normalized.contains("i can take this on")
            || normalized.contains("i can take this")
            || normalized.contains("let me take this issue")
            || normalized.contains("let me take this")
            || normalized.contains("i can take care")
            || normalized.contains("i ll take care")
            || normalized.contains("can i take this")
            || normalized.contains("could i take this")
            || normalized.contains("can i have a shot")
            || normalized.contains("give this one a try")
            || normalized.contains("give it a try")
            || normalized.contains("i would love to fix")
            || normalized.contains("pick this up")
            || normalized.contains("picked this up")
            || normalized.contains("take this up")
            || normalized.contains("can you assign me")
            || normalized.contains("assign me this issue")
            || normalized.contains("please assign")
            || normalized.contains("please assign me")
            || normalized.contains("puedo trabajar en este")
            || normalized.contains("puedo tomar este")
            || normalized.contains("yo quiero contribuir"),
        working: normalized.contains("working on this")
            || normalized.contains("i am working on this")
            || normalized.contains("i m working on this")
            || normalized.contains("i will look into it")
            || normalized.contains("i ll look into it")
            || normalized.contains("i was able to replicate"),
        fix_submitted: normalized.contains("fix submitted in pr")
            || normalized.contains("submitted in pr")
            || normalized.contains("opened a pr")
            || normalized.contains("created pr")
            || normalized.contains("created a pr")
            || normalized.contains("created pull request")
            || normalized.contains("i have created pr")
            || normalized.contains("i have created a pr")
            || normalized.contains("i opened a pr")
            || normalized.contains("i opened pull request")
            || normalized.contains("i filed a pr")
            || normalized.contains("filed a pr")
            || normalized.contains("filed pr")
            || normalized.contains("filed pull request")
            || normalized.contains("i submitted a pr")
            || normalized.contains("i submitted pull request")
            || normalized.contains("pull request submitted")
            || normalized.contains("submitted a pull request")
            || normalized.contains("submitted pull request")
            || normalized.contains("submitted the pr")
            || normalized.contains("submitted the pull request")
            || normalized.contains("sent a pull request")
            || normalized.contains("sent pull request")
            || normalized.contains("sent a pr")
            || normalized.contains("submitted before")
            || normalized.contains("pr submitted")
            || normalized.contains("pr is open")
            || normalized.contains("pr is ready")
            || normalized.contains("pr solves this")
            || normalized.contains("pr should solve this")
            || normalized.contains("pull request solves this")
            || normalized.contains("pull request should solve this")
            || normalized.contains("pr moved back to draft")
            || normalized.contains("moved back to draft")
            || normalized.contains("draft pr")
            || normalized.contains("awaiting a pr")
            || normalized.contains("awaiting pr")
            || normalized.contains("pr i ve got")
            || normalized.contains("pr i've got")
            || normalized.contains("take a look at the pr")
            || normalized.contains("reviewed the pr")
            || normalized.contains("reviews on the pr")
            || normalized.contains("should i make a pr")
            || normalized.contains("here is my contrib")
            || normalized.contains("left the reviews on the commit")
            || normalized.contains("redone my changes")
            || normalized.contains("will be fixing this in my pull request")
            || normalized.contains("fixing this in my pull request")
            || normalized.contains("fixed by pr")
            || normalized.contains("fixed by #")
            || normalized.contains("confirmed fixed")
            || normalized.contains("no longer reproduce")
            || normalized.contains("should we close this issue since fixed")
            || (body.contains("/pull/") && normalized.contains("filed"))
            || (body.contains("/pull/") && normalized.contains("solves this")),
    }
}

pub fn is_issue_finder_projection_comment(body: &str) -> bool {
    let normalized = normalize(body);
    normalized.contains("issue finder projection")
        || normalized.contains("issue finder tracking comment")
        || normalized.contains("issue finder progress comment")
        || normalized.contains("issue finder clarification comment")
        || normalized.contains("issue finder final comment")
        || body.contains("<!-- issue-finder:")
        || normalized == "i am tracking this issue and preparing a fix attempt"
}

fn has_phrase(normalized: &str, phrase: &str) -> bool {
    normalized.split_whitespace().any(|word| word == phrase)
}

pub fn competition_band(points: i32) -> CompetitionBand {
    match points {
        value if value <= 1 => CompetitionBand::Clear,
        value if value <= 3 => CompetitionBand::Light,
        value if value <= 7 => CompetitionBand::Contested,
        _ => CompetitionBand::Saturated,
    }
}

fn is_open(state: Option<&str>) -> bool {
    state
        .map(|value| value.eq_ignore_ascii_case("open"))
        .unwrap_or(false)
}

impl fmt::Display for CompetitionBand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Clear => "clear",
            Self::Light => "light",
            Self::Contested => "contested",
            Self::Saturated => "saturated",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        assess_competition, detect_comment_competition_markers, CommentCompetitionMarkers,
        CompetitionBand, TimelineIssueReference,
    };

    #[test]
    fn bands_competition_points() {
        let refs = vec![
            TimelineIssueReference {
                source_ref: "issue:timeline.0".to_string(),
                state: Some("open".to_string()),
                is_pull_request: true,
                created_at: Some("2026-01-01T00:00:00Z".to_string()),
            },
            TimelineIssueReference {
                source_ref: "issue:timeline.1".to_string(),
                state: Some("closed".to_string()),
                is_pull_request: true,
                created_at: Some("2026-01-02T00:00:00Z".to_string()),
            },
        ];
        let comments = vec!["/attempt\nWorking on this".to_string()];
        let facts = assess_competition(&refs, &comments, Vec::new());
        assert_eq!(facts.competition_points, 6);
        assert_eq!(facts.competition_band, CompetitionBand::Contested);
        assert_eq!(
            facts.latest_competition_at,
            Some("2026-01-02T00:00:00Z".to_string())
        );
    }

    #[test]
    fn detects_attempt_and_claim_markers() {
        let markers = detect_comment_competition_markers("/claim\nFix submitted in PR #12");
        assert!(markers.claim);
        assert!(markers.fix_submitted);
        assert!(!markers.working);
    }

    #[test]
    fn detects_natural_language_claim_markers() {
        for body in [
            "/assign",
            "hey, would it be ok if I picked this up?",
            "Can I work on this?",
            "Can I contribute to this one?",
            "Hi! I'd like to take a look.",
            "I'd like to fix this. Please assign me.",
            "I am ready to contribute. Please assign this issue to me.",
            "Can you assign me this issue?",
            "I would like to take this issue.",
            "I would like to be assigned.",
            "I can take care of the bug.",
            "I can take this on.",
            "Can I have a shot at this?",
            "Let me take this issue. Please assign.",
            "I want to pick this, is it available?",
            "If this problem is still open, can I work on it?",
            "I would like to work on this issue.",
            "If not completed, I would like to work on it.",
            "If yes, I would like to take it up.",
            "I would give this one a try.",
            "I would love to fix the typing cursor issue.",
            "Hi! I'm interested in contributing to this issue.",
            "If contributions are welcome, I'd be happy to implement this.",
            "Puedo trabajar en este Issue?",
            "Yo quiero contribuir con este issue",
        ] {
            let markers = detect_comment_competition_markers(body);
            assert!(markers.claim, "{body}");
        }
    }

    #[test]
    fn ignores_issue_finder_projection_comments_as_competition() {
        for body in [
            "<!-- issue-finder:tracking_comment -->\nI am tracking this issue and preparing a fix attempt.",
            "<!-- issue-finder:progress_comment -->\nI am working on this and will report back.",
            "<!-- issue-finder:clarification_comment -->\nCould a maintainer clarify the expected behavior?",
            "<!-- issue-finder:final_comment -->\nI opened a PR with the fix.",
            "Issue Finder progress comment: I am working on this.",
            "Issue Finder clarification comment: Could a maintainer clarify the expected behavior?",
            "Issue Finder final comment: I opened a PR with the fix.",
            "I am tracking this issue and preparing a fix attempt.",
        ] {
            let markers = detect_comment_competition_markers(body);
            assert_eq!(markers, CommentCompetitionMarkers::default());
        }

        let facts = assess_competition(
            &[],
            &[
                "<!-- issue-finder:tracking_comment -->\nI am tracking this issue and preparing a fix attempt.".to_string(),
            ],
            Vec::new(),
        );
        assert_eq!(facts.competition_points, 0);
        assert_eq!(facts.competition_band, CompetitionBand::Clear);
    }

    #[test]
    fn detects_natural_language_working_markers() {
        for body in [
            "I will look into it",
            "I'll look into it.",
            "I was able to replicate the issue locally.",
        ] {
            let markers = detect_comment_competition_markers(body);
            assert!(markers.working, "{body}");
        }
    }

    #[test]
    fn detects_fixed_or_no_longer_reproducible_markers() {
        for body in [
            "This issue has been fixed by PR #3457.",
            "I have created a PR for this task.",
            "I opened a PR for this task.",
            "Filed https://github.com/kubernetes-sigs/kind/pull/1176",
            "I submitted a pull request for the fix.",
            "I have sent a pull request for config.go; kindly review it.",
            "The PR moved back to Draft while I update tests.",
            "I think https://github.com/dapr/dapr/pull/6281 solves this now.",
            "Will be fixing this in my pull request.",
            "I am awaiting a PR from the contributor.",
            "Would you mind taking a look at the PR I've got?",
            "I reviewed the PR and left comments.",
            "Here is my contrib for nested fields.",
            "Confirmed fixed by #3457.",
            "I can no longer reproduce this on current main.",
            "Should we close this issue since fixed?",
        ] {
            let markers = detect_comment_competition_markers(body);
            assert!(markers.fix_submitted, "{body}");
        }
    }
}
