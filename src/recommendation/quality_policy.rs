use chrono::{DateTime, Utc};

use crate::competition::{is_issue_finder_projection_comment, CompetitionBand};
use crate::github_enrichment::EnrichedIssue;
use crate::scoring::normalize;
use crate::value_scoring::{GateStatus, RecommendationCategory, RiskTag, ValueAssessment};

use super::model::RecommendationVisibility;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityPolicyAssessment {
    pub freshness_cap: Option<i32>,
    pub penalty: i32,
    pub visibility: Option<RecommendationVisibility>,
    pub reasons: Vec<String>,
}

pub fn assess_quality_policy(
    value: &ValueAssessment,
    enriched: &EnrichedIssue,
) -> QualityPolicyAssessment {
    let mut assessment = QualityPolicyAssessment {
        freshness_cap: None,
        penalty: 0,
        visibility: None,
        reasons: Vec::new(),
    };

    apply_noise_constraints(enriched, &mut assessment);
    apply_trust_constraints(value, &mut assessment);
    apply_profile_constraints(value, &mut assessment);
    apply_low_impact_constraints(value, enriched, &mut assessment);
    apply_competition_constraints(enriched, &mut assessment);
    apply_low_depth_constraints(value, enriched, &mut assessment);
    apply_scope_constraints(value, enriched, &mut assessment);

    assessment
}

fn apply_trust_constraints(value: &ValueAssessment, assessment: &mut QualityPolicyAssessment) {
    if value.risk_tags.contains(&RiskTag::LowTrustRepo)
        || value.risk_tags.contains(&RiskTag::ForkStarAnomaly)
        || value.risk_tags.contains(&RiskTag::MarketplaceNoise)
    {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 220;
        assessment
            .reasons
            .push("Quality policy: low-trust repository signals are hidden".to_string());
    }
}

fn apply_profile_constraints(value: &ValueAssessment, assessment: &mut QualityPolicyAssessment) {
    if value.profile_fit_score < 60
        || value.gates.profile_fit.status == GateStatus::HardFail
        || value.risk_tags.contains(&RiskTag::ProfileMismatch)
    {
        cap_freshness(assessment, 8);
        assessment.penalty += 180;
        if value.profile_fit_score < 60
            || value.recommendation_category == RecommendationCategory::NeedsTriage
            || value.recommendation_category == RecommendationCategory::ContestedOrLowTrust
        {
            assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        }
        assessment.reasons.push(
            "Quality policy: profile mismatch or very weak profile fit is hidden".to_string(),
        );
    } else if value.profile_fit_score < 70 {
        cap_freshness(assessment, 12);
        assessment.penalty += 220;
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.reasons.push(
            "Quality policy: acceptable but weak profile fit is hidden from the feed".to_string(),
        );
    }
}

fn apply_noise_constraints(enriched: &EnrichedIssue, assessment: &mut QualityPolicyAssessment) {
    let text = normalized_issue_and_repo(enriched);
    if is_dashboard_noise(&text) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 220;
        assessment
            .reasons
            .push("Quality policy: dashboard or bot-maintained issue is hidden".to_string());
        return;
    }

    if is_toy_no_code_task(enriched, &text) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 180;
        assessment
            .reasons
            .push("Quality policy: toy or no-code task is hidden".to_string());
    }
}

fn apply_low_impact_constraints(
    value: &ValueAssessment,
    enriched: &EnrichedIssue,
    assessment: &mut QualityPolicyAssessment,
) {
    if value.risk_tags.contains(&RiskTag::LowImpactRepo) && enriched.repository.stars < 50 {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 180;
        assessment
            .reasons
            .push("Quality policy: very low-impact repository is hidden".to_string());
        return;
    }

    if value.risk_tags.contains(&RiskTag::LowImpactRepo)
        && value.recommendation_category == RecommendationCategory::NeedsTriage
        && issue_age_days(enriched).is_some_and(|age_days| age_days > 365)
    {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 150;
        assessment
            .reasons
            .push("Quality policy: old low-impact needs-triage issue is hidden".to_string());
        return;
    }

    if value.risk_tags.contains(&RiskTag::LowImpactRepo)
        && value.risk_tags.contains(&RiskTag::WeakValidationPath)
    {
        cap_freshness(assessment, 16);
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 160;
        assessment.reasons.push(
            "Quality policy: low-impact repo with weak validation path is hidden".to_string(),
        );
    } else if value.risk_tags.contains(&RiskTag::LowImpactRepo) {
        cap_freshness(assessment, 28);
        assessment.penalty += 80;
        assessment
            .reasons
            .push("Quality policy: low-impact repo receives a feed penalty".to_string());
    }
}

fn apply_competition_constraints(
    enriched: &EnrichedIssue,
    assessment: &mut QualityPolicyAssessment,
) {
    let text = normalized_issue_and_comments(enriched);
    if is_answered_support_question(enriched, &text) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 160;
        assessment
            .reasons
            .push("Quality policy: answered support question is hidden".to_string());
        return;
    }

    let has_open_pr = enriched.competition.open_pr_refs > 0
        || enriched.competition.fix_submitted_comments > 0
        || text.contains("pr opened to address this")
        || text.contains("pull request opened to address this")
        || text.contains("opened pr to address this")
        || text.contains("fix submitted in pr")
        || text.contains("submitted in pr")
        || text.contains("submitted a pr")
        || text.contains("i ve submitted a pr")
        || text.contains("i have submitted a pr")
        || text.contains("created pr")
        || text.contains("created a pr")
        || text.contains("created pull request")
        || text.contains("i have created pr")
        || text.contains("i have created a pr")
        || text.contains("opened a pr")
        || text.contains("awaiting a pr")
        || text.contains("awaiting pr")
        || text.contains("pr i ve got")
        || text.contains("take a look at the pr")
        || text.contains("should i make a pr")
        || text.contains("here is my contrib")
        || text.contains("left the reviews on the commit")
        || text.contains("redone my changes")
        || text.contains("will be fixing this in my pull request")
        || text.contains("fixing this in my pull request")
        || text.contains("pull request submitted")
        || text.contains("pull request in progress")
        || text.contains("pr in progress")
        || text.contains("raised my pr")
        || text.contains("pr fixing this")
        || text.contains("there is a pr merged")
        || text.contains("pr merged")
        || text.contains("pr is good to merge")
        || text.contains("pull request is good to merge")
        || text.contains("yield to that one")
        || text.contains("fixed by pr")
        || text.contains("fixed by #")
        || text.contains("fixing this is")
        || text.contains("confirmed fixed")
        || text.contains("looks like this issue can be closed")
        || text.contains("issue can be closed")
        || text.contains("can be closed")
        || text.contains("no longer reproduce")
        || text.contains("should we close this issue since fixed");
    let claimed = enriched.competition.attempt_comments > 0
        || enriched.competition.claim_comments > 0
        || enriched.competition.working_comments > 0
        || text.contains("i d love to work on this")
        || text.contains("i would love to work on this")
        || text.contains("interested in contributing")
        || text.contains("interested in working on this")
        || text.contains("external contributions be welcome")
        || text.contains("happy to implement")
        || text.contains("i d like to work on this")
        || text.contains("i would like to work on this")
        || text.contains("would like to work on this")
        || text.contains("would love to work on this")
        || text.contains("i d like to take a look")
        || text.contains("i would like to take a look")
        || text.contains("i d like to fix this")
        || text.contains("i would like to fix this")
        || text.contains("i d like to take this issue")
        || text.contains("i would like to take this issue")
        || text.contains("i would like to be assigned")
        || text.contains("would like to be assigned")
        || text.contains("i want to work on this")
        || text.contains("i want to take this")
        || text.contains("i want to pick this")
        || text.contains("want to pick this")
        || text.contains("can i work on it")
        || text.contains("could i work on it")
        || text.contains("take it up")
        || text.contains("take this issue up")
        || text.contains("i can take care")
        || text.contains("i ll take care")
        || text.contains("give this one a try")
        || text.contains("give it a try")
        || text.contains("i would love to fix")
        || text.contains("can i work on this")
        || text.contains("could i work on this")
        || text.contains("shall i work on this")
        || text.contains("could i give it a try")
        || text.contains("assigned to me")
        || text.contains("assign this issue to me")
        || text.contains("assign me this issue")
        || text.contains("could this issue be assigned")
        || text.contains("can i be assigned")
        || text.contains("could i be assigned")
        || text.contains("can i get this issue")
        || text.contains("can you assign me")
        || text.contains("i m working on this")
        || text.contains("i am working on this")
        || text.contains("i will look into it")
        || text.contains("i ll look into it")
        || text.contains("i was able to replicate")
        || text.contains("yes go ahead")
        || text.contains("go ahead @")
        || text.contains("sure go for it")
        || text.contains("picking this")
        || text.contains("pick this up")
        || text.contains("picked this up")
        || text.contains("take this up")
        || text.contains("claims the bounty")
        || text.contains("please assign me")
        || text.contains("puedo trabajar en este")
        || text.contains("puedo tomar este")
        || text.contains("yo quiero contribuir")
        || text.contains("recommend holding off")
        || text.contains("recommend hold off")
        || text.contains("holding off on this")
        || text.contains("hold off on this")
        || text.contains("hold off for now")
        || text.contains("feel free to fork")
        || text.contains("looking forward to your contribution");

    if has_open_pr {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 200;
        assessment
            .reasons
            .push("Quality policy: issue appears to have an open or submitted PR".to_string());
        return;
    }

    if enriched.competition.competition_band == CompetitionBand::Contested
        || enriched.competition.competition_band == CompetitionBand::Saturated
    {
        assessment.penalty += 90;
        assessment
            .reasons
            .push("Quality policy: contested issue is strongly deprioritized".to_string());
    }

    if claimed {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 140;
        assessment.reasons.push(
            "Quality policy: issue appears claimed or already guided to a contributor".to_string(),
        );
    }
}

fn apply_low_depth_constraints(
    value: &ValueAssessment,
    enriched: &EnrichedIssue,
    assessment: &mut QualityPolicyAssessment,
) {
    let text = normalized_issue_and_repo(enriched);
    let text_with_comments = normalized_issue_and_comments(enriched);
    if value.recommendation_category == RecommendationCategory::FilteredLowDepth {
        assessment.visibility = Some(RecommendationVisibility::HiddenFiltered);
        assessment.penalty += 160;
        assessment
            .reasons
            .push("Quality policy: value model filtered this low-depth issue".to_string());
        return;
    }

    if is_trivial_docs_task(&text) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 160;
        assessment
            .reasons
            .push("Quality policy: low-depth documentation or wording task is hidden".to_string());
        return;
    }

    if is_comment_guided_docs_task(&text_with_comments) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 150;
        assessment.reasons.push(
            "Quality policy: maintainer-guided documentation-only task is hidden".to_string(),
        );
        return;
    }

    if is_docs_polish_task(&text) {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 120;
        assessment
            .reasons
            .push("Quality policy: documentation polish task is hidden".to_string());
    }
}

fn apply_scope_constraints(
    value: &ValueAssessment,
    enriched: &EnrichedIssue,
    assessment: &mut QualityPolicyAssessment,
) {
    let text = normalized_issue_and_repo(enriched);
    let broad_audit_needs_triage = is_large_audit_task(&text)
        && (value.recommendation_category == RecommendationCategory::NeedsTriage
            || value.risk_tags.contains(&RiskTag::HighTriageLoad)
            || value.risk_tags.contains(&RiskTag::WeakValidationPath)
            || value.risk_tags.contains(&RiskTag::ProfileMismatch));
    if broad_audit_needs_triage {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 180;
        assessment.reasons.push(
            "Quality policy: broad audit or campaign task is hidden from the feed".to_string(),
        );
        return;
    }

    let old_broad_needs_triage = value.recommendation_category
        == RecommendationCategory::NeedsTriage
        && issue_age_days(enriched).is_some_and(|age_days| age_days > 365)
        && (value.risk_tags.contains(&RiskTag::ScopeRisk) || is_broad_feature_request(&text));
    if old_broad_needs_triage {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.penalty += 170;
        assessment
            .reasons
            .push("Quality policy: old broad needs-triage feature request is hidden".to_string());
        return;
    }

    if value.risk_tags.contains(&RiskTag::ScopeRisk) || is_large_audit_task(&text) {
        cap_freshness(assessment, 18);
        assessment.penalty += 75;
        assessment
            .reasons
            .push("Quality policy: broad audit or campaign task needs scoping first".to_string());
    }
}

fn is_answered_support_question(enriched: &EnrichedIssue, text: &str) -> bool {
    let title = normalize(&enriched.issue.title);
    let support_question = title.starts_with("how to")
        || title.starts_with("how can")
        || title.starts_with("question")
        || text.contains("feature request question")
        || text.contains("is there any way")
        || text.contains("how do i");
    support_question
        && contains_any(
            text,
            &[
                "i have found the answer",
                "i found the answer",
                "i have solved this problem",
                "i ve solved this problem",
                "i solved this problem",
                "found the answer",
                "that answers my question",
                "this answers my question",
            ],
        )
}

fn is_comment_guided_docs_task(text: &str) -> bool {
    contains_any(
        text,
        &[
            "doc fix",
            "docs fix",
            "documentation entry",
            "documentation-only",
            "documentation only",
        ],
    ) && contains_any(
        text,
        &[
            "send a pr",
            "send a pull request",
            "feel free to send",
            "would be really useful for new users",
        ],
    )
}

fn cap_freshness(assessment: &mut QualityPolicyAssessment, cap: i32) {
    assessment.freshness_cap = Some(
        assessment
            .freshness_cap
            .map(|existing| existing.min(cap))
            .unwrap_or(cap),
    );
}

fn issue_age_days(enriched: &EnrichedIssue) -> Option<i64> {
    let created_at = DateTime::parse_from_rfc3339(&enriched.issue.created_at).ok()?;
    Some((Utc::now() - created_at.with_timezone(&Utc)).num_days())
}

fn is_trivial_docs_task(text: &str) -> bool {
    (contains_any(
        text,
        &[
            "readme",
            "manual",
            "user guide",
            "claude md",
            "contributing md",
        ],
    ) && contains_any(
        text,
        &[
            "english wording",
            "clarity grammar",
            "grammar",
            "natural wording",
            "duplicate api entries",
            "remove duplicate",
            "single line fix",
            "one line fix",
            "incorrect signature",
            "documentation error",
            "crate name fix",
            "line 284",
        ],
    )) || contains_any(
        text,
        &[
            "add documentation",
            "add docs",
            "trivial 5 minutes",
            "trivial 10 minutes",
            "estimated effort trivial",
            "pure docs formatting",
            "documentation formatting error",
            "documentation only no behavior change",
            "doc comments still describe",
            "stale wording in",
        ],
    )
}

fn is_docs_polish_task(text: &str) -> bool {
    contains_any(
        text,
        &[
            "docs should include",
            "demo video",
            "document usage in readme",
            "documentation and visual testing",
            "improve readme",
            "installing and using",
            "please add a further bullet point",
            "review the english",
            "improve english wording",
            "remove stale todo",
            "manual and public project description",
            "readme completeness",
            "license footer",
            "set up storybook",
            "storybook for component documentation",
            "table of contents",
            "sample directory with jupyter notebook",
            "interactive learning resources",
            "hands-on examples",
            "tutorial structure",
            "onboarding process",
            "ui screenshots",
            "write contributing",
            "write backend-template",
        ],
    )
}

fn is_large_audit_task(text: &str) -> bool {
    contains_any(
        text,
        &[
            "audit",
            "110 gaps",
            "127 publishable crates",
            "2000 print",
            "2 000 print",
            "phase 1 triage",
            "phase 2 remediation",
            "large volume",
            "not a mechanical fix",
            "requires judgment",
            "sampling phase",
            "campaign",
        ],
    )
}

fn is_broad_feature_request(text: &str) -> bool {
    contains_any(
        text,
        &[
            "epic:",
            "epic domain",
            "research & feasibility analysis",
            "there should be a ui interface",
            "this can be feature gateable",
        ],
    )
}

fn is_dashboard_noise(text: &str) -> bool {
    contains_any(
        text,
        &[
            "dependency dashboard",
            "renovate",
            "detected dependencies",
            "package updates generated",
            "view abandoned dependencies",
        ],
    )
}

fn is_toy_no_code_task(enriched: &EnrichedIssue, text: &str) -> bool {
    let repo_stars = enriched.repository.stars;
    let low_trust_repo = repo_stars < 50;
    let add_simple_asset = contains_any(
        text,
        &[
            "add a copy profile link button",
            "add a yaml to json converter",
            "add yaml to json converter",
            "converter card",
            "currency converter cli script",
            "build currency converter cli",
            "build a beginner currency converter",
            "currency converter with fixed rates",
            "build quiz game cli",
            "quiz game cli",
            "add ten beginner learning resources",
            "beginner learning resources",
            "learning resources about ai agents",
            "cuốn sách ai",
            "sách ai",
        ],
    );
    let no_code_shape = contains_any(
        text,
        &[
            "beginner friendly",
            "collection of beginner python scripts",
            "keep the page consistent",
            "learning resources",
            "resource list",
            "use any public api and print",
            "small open source portfolio app",
            "small collection of developer tools",
        ],
    );
    low_trust_repo && (add_simple_asset || no_code_shape)
}

fn normalized_issue_and_repo(enriched: &EnrichedIssue) -> String {
    normalize(&format!(
        "{} {} {} {} {}",
        enriched.issue.title,
        enriched.issue.body,
        enriched.issue.labels.join(" "),
        enriched.repository.description,
        enriched.repository.topics.join(" ")
    ))
}

fn normalized_issue_and_comments(enriched: &EnrichedIssue) -> String {
    normalize(&format!(
        "{} {} {}",
        enriched.issue.title,
        enriched.issue.body,
        enriched
            .comments
            .iter()
            .filter(|comment| !is_issue_finder_projection_comment(&comment.body_excerpt))
            .map(|comment| comment.body_excerpt.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}
