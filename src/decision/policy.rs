//! Deterministic scout policy over GitHub facts and validated semantic answers.
//! No legacy textual classification, comment counters, or risk scores enter this path.

use chrono::DateTime;

use crate::config::ProfileConfig;
use crate::github_enrichment::EnrichedIssue;
use crate::recommendation::quality_policy::QualityPolicyAssessment;
use crate::recommendation::RecommendationVisibility;
use crate::value_model::{
    GateBand, GateStatus, GateVerdict, RecommendationCategory, RiskTag, ScoreBand, ValueAssessment,
    ValueEvidence, ValueGates, ValueScores,
};

use super::questions::{
    ContributionSignal, DescriptionQuality, MaintainerSignal, PreferenceMatch, Scope,
    SemanticAnswers, TaskType, VerificationClues,
};

pub fn assess(
    enriched: &EnrichedIssue,
    answers: Option<&SemanticAnswers>,
    _profile: &ProfileConfig,
) -> ValueAssessment {
    let unknown = SemanticAnswers::default();
    let answers = answers.unwrap_or(&unknown);
    let mut missing_evidence = enriched.warnings.clone();
    missing_evidence.extend(enriched.competition.warnings.clone());
    if let Some(snapshot) = &enriched.decision {
        if let Some(error) = &snapshot.error {
            missing_evidence.push(format!("Decision model: {error}"));
        }
        if let Some(evidence) = &snapshot.evidence {
            missing_evidence.extend(evidence.warnings.clone());
            if evidence.github_status.issue_state.is_none() {
                missing_evidence
                    .push("GitHub issue state was not fetched for this snapshot".into());
            }
            if evidence.github_status.assignees.is_none() {
                missing_evidence.push("GitHub assignees were not fetched for this snapshot".into());
            }
        }
    }
    let status = github_status(enriched);
    let availability = enriched.availability.as_ref();
    if let Some(availability) = availability {
        missing_evidence.extend(availability.uncertainties.clone());
        if !availability.checks_complete() {
            missing_evidence.push(
                "Fresh GitHub availability checks are incomplete; availability remains uncertain"
                    .into(),
            );
        } else if !availability.is_known_unavailable()
            && !availability.has_strong_open_competition()
            && !availability.has_merged_resolution_evidence()
            && !availability.fresh_fix_available()
        {
            missing_evidence.push(
                "GitHub PR leads require checking their relationship to the issue and target branch".into(),
            );
        }
    }
    let closed = issue_closed(enriched);
    let assigned = availability.map_or_else(
        || status.is_some_and(|s| s.assignees.as_ref().is_some_and(|users| !users.is_empty())),
        |a| a.assignees.as_ref().is_some_and(|users| !users.is_empty()),
    );
    let open_pr_count = strong_open_pr_count(enriched);
    let unavailable = availability.is_some_and(|a| a.is_known_unavailable());
    let merged_resolution = availability.is_some_and(|a| a.has_merged_resolution_evidence());
    let availability_uncertain = availability.is_some_and(|a| !a.fresh_fix_available());
    for (id, missing) in [
        ("task_type", answers.task_type.is_none()),
        ("description_quality", answers.description_quality.is_none()),
        ("contribution_signal", answers.contribution_signal.is_none()),
        ("scope", answers.scope.is_none()),
        ("preference_match", answers.preference_match.is_none()),
        ("verification_clues", answers.verification_clues.is_none()),
        ("maintainer_signal", answers.maintainer_signal.is_none()),
    ] {
        if missing {
            missing_evidence.push(format!(
                "Decision model {id} was not answered; semantic state is unknown"
            ));
        }
    }
    if matches!(
        answers.contribution_signal,
        Some(
            ContributionSignal::NotObserved
                | ContributionSignal::Unknown
                | ContributionSignal::Conflicting
        ) | None
    ) {
        missing_evidence.push("Available comments do not establish that nobody is working; assess must read current discussion".into());
    }
    if answers.contribution_signal == Some(ContributionSignal::FixClaimed) {
        missing_evidence.push(
            "A participant claims a fix; verify the latest issue and PR facts in assess".into(),
        );
    }
    if answers.verification_clues != Some(VerificationClues::Present) {
        missing_evidence.push("Verify a concrete result or reproduction path in assess".into());
    }

    let influence = repository_influence(enriched);
    let profile_fit = match answers.preference_match {
        Some(PreferenceMatch::Matches) => 90,
        Some(PreferenceMatch::Mismatch) => 20,
        Some(PreferenceMatch::NotSpecified) => 70,
        Some(PreferenceMatch::Unclear) | None => 50,
    };
    let mut execution = match answers.description_quality {
        Some(DescriptionQuality::Clear) => 70,
        Some(DescriptionQuality::Partial) => 50,
        Some(DescriptionQuality::Unclear) => 30,
        None => 50,
    };
    if answers.verification_clues == Some(VerificationClues::Present) {
        execution += 15;
    }
    if answers.scope == Some(Scope::Bounded) {
        execution += 10;
    }
    let needs_scoping = matches!(
        answers.scope,
        Some(Scope::DesignNeeded | Scope::Broad | Scope::Unclear)
    ) || matches!(
        answers.maintainer_signal,
        Some(MaintainerSignal::ClarificationNeeded | MaintainerSignal::Deferred)
    );
    if needs_scoping {
        execution = execution.min(65);
    }
    let maintainer = match answers.maintainer_signal {
        Some(MaintainerSignal::Encouraged) => 90,
        Some(MaintainerSignal::ClarificationNeeded) => 55,
        Some(MaintainerSignal::Deferred) => 25,
        _ => 50,
    };
    let non_task = matches!(
        answers.task_type,
        Some(TaskType::SupportQuestion | TaskType::OpenDiscussion)
    );
    let factual_low_trust = repository_low_trust(enriched);
    let low_trust = factual_low_trust || availability.is_some_and(|a| a.archived == Some(true));
    // Discussion expressions request verification; only GitHub facts establish competition.
    let contested = assigned || open_pr_count > 0;
    let discussion_needs_check = matches!(
        answers.contribution_signal,
        Some(
            ContributionSignal::Working
                | ContributionSignal::FixClaimed
                | ContributionSignal::Conflicting
        )
    );
    let category =
        if closed || unavailable || merged_resolution || low_trust || assigned || open_pr_count > 0
        {
            RecommendationCategory::ContestedOrLowTrust
        } else if non_task {
            RecommendationCategory::FilteredLowDepth
        } else if answers.task_type != Some(TaskType::ConcreteChange)
            || answers.description_quality == Some(DescriptionQuality::Unclear)
            || matches!(
                answers.preference_match,
                Some(PreferenceMatch::Mismatch | PreferenceMatch::Unclear) | None
            )
        {
            RecommendationCategory::NeedsTriage
        } else if needs_scoping
            || availability_uncertain
            || discussion_needs_check
            || execution < 70
            || answers.scope.is_none()
            || answers.description_quality != Some(DescriptionQuality::Clear)
            || matches!(
                answers.contribution_signal,
                Some(ContributionSignal::Unknown) | None
            )
        {
            RecommendationCategory::HighValueNeedsScoping
        } else if influence >= 70 {
            RecommendationCategory::HighValueReady
        } else {
            RecommendationCategory::NicheButActionable
        };

    let mut risk_tags = Vec::new();
    if low_trust {
        risk_tags.push(RiskTag::LowTrustRepo);
    }
    if contested {
        risk_tags.push(RiskTag::CompetitionContested);
    }
    if needs_scoping {
        risk_tags.push(RiskTag::ScopeRisk);
    }
    if answers.preference_match == Some(PreferenceMatch::Mismatch) {
        risk_tags.push(RiskTag::ProfileMismatch);
    }
    if answers.verification_clues == Some(VerificationClues::Absent) {
        risk_tags.push(RiskTag::WeakValidationPath);
    }
    // Risk tags explain the category. They are not separately charged again.
    let scores = ValueScores {
        repo_influence_score: influence,
        profile_fit_score: profile_fit,
        execution_quality_score: execution,
        maintainer_signal_score: maintainer,
        freshness_score: freshness_score(enriched),
        risk_score: 0,
    };
    let final_rank_score = (influence * 20
        + profile_fit * 30
        + execution * 35
        + maintainer * 10
        + scores.freshness_score * 5)
        / 100;
    let gates = ValueGates {
        low_depth: verdict(
            if non_task {
                GateStatus::HardFail
            } else if answers.task_type.is_none() {
                GateStatus::SoftFail
            } else {
                GateStatus::Pass
            },
            if non_task {
                GateBand::Weak
            } else {
                GateBand::Acceptable
            },
            format!("Decision model task type {:?}", answers.task_type),
        ),
        repo_influence: verdict(
            if factual_low_trust {
                GateStatus::HardFail
            } else if influence >= 70 {
                GateStatus::Pass
            } else {
                GateStatus::SoftFail
            },
            if influence >= 70 {
                GateBand::Strong
            } else {
                GateBand::Acceptable
            },
            format!(
                "GitHub repository facts: {} stars, archived={}",
                enriched.repository.stars, enriched.repository.archived
            ),
        ),
        competition: verdict(
            if contested {
                GateStatus::SoftFail
            } else {
                GateStatus::Pass
            },
            if contested {
                GateBand::Contested
            } else {
                GateBand::Acceptable
            },
            format!(
                "{} actual open PR references; Decision model contribution {:?}",
                open_pr_count, answers.contribution_signal
            ),
        ),
        profile_fit: verdict(
            if answers.preference_match == Some(PreferenceMatch::Mismatch) {
                GateStatus::SoftFail
            } else {
                GateStatus::Pass
            },
            if profile_fit >= 70 {
                GateBand::Acceptable
            } else {
                GateBand::Weak
            },
            format!(
                "Decision model preference match {:?}",
                answers.preference_match
            ),
        ),
    };
    let explanation = vec![
        format!("Decision model: task={:?}, description={:?}, scope={:?}", answers.task_type, answers.description_quality, answers.scope),
        format!("Decision model discussion expression={:?}, preference={:?}", answers.contribution_signal, answers.preference_match),
        "Semantic answers guide selection; reproduction, fix verification and execution remain with the main Agent".into(),
        format!("GitHub facts: issue_closed={closed}, assigned={assigned}, open_pr_count={open_pr_count}, merged_resolution_evidence={merged_resolution}"),
    ];
    let evidence = explanation
        .iter()
        .map(|summary| ValueEvidence {
            summary: summary.clone(),
            evidence_refs: vec!["decision:snapshot".into()],
        })
        .collect();
    ValueAssessment {
        final_rank_score,
        category,
        recommendation_category: category,
        gates,
        scores,
        risk_tags,
        evidence,
        missing_evidence,
        explanation,
        attention_score: influence,
        execution_score: execution,
        profile_fit_score: profile_fit,
        risk_penalty: 0,
        attention_band: band(influence),
        execution_band: band(execution),
        signals: Vec::new(),
    }
}

/// Visibility consumes facts and typed answers directly. No legacy tags or
/// counters are reinterpreted and no semantic penalty is applied a second time.
pub fn quality(
    enriched: &EnrichedIssue,
    answers: Option<&SemanticAnswers>,
) -> QualityPolicyAssessment {
    let mut assessment = QualityPolicyAssessment {
        freshness_cap: None,
        penalty: 0,
        visibility: None,
        reasons: Vec::new(),
    };
    if issue_closed(enriched) {
        assessment.visibility = Some(RecommendationVisibility::HiddenDone);
        assessment
            .reasons
            .push("GitHub confirms the issue is closed".into());
        return assessment;
    }
    let availability = enriched.availability.as_ref();
    let reason = if availability.is_some_and(|a| a.is_known_unavailable()) {
        Some("Fresh GitHub facts confirm the issue is unavailable for a new contribution")
    } else if availability.is_some_and(|a| a.has_merged_resolution_evidence()) {
        Some("GitHub confirms merged resolution evidence; inspect current code before treating this as a fresh fix")
    } else if enriched.repository.archived {
        Some("GitHub confirms the repository is archived")
    } else if repository_low_trust(enriched) {
        Some("GitHub repository metrics indicate anomalous influence or issue volume")
    } else if strong_open_pr_count(enriched) > 0 {
        Some("GitHub confirms a linked PR is open; prefer an uncontested issue")
    } else if answers.is_some_and(|a| a.preference_match == Some(PreferenceMatch::Mismatch)) {
        Some("Decision model identifies a mismatch with the supplied task preferences")
    } else {
        None
    };
    if let Some(reason) = reason {
        assessment.visibility = Some(RecommendationVisibility::HiddenQuality);
        assessment.reasons.push(reason.into());
    }
    assessment
}

fn issue_closed(enriched: &EnrichedIssue) -> bool {
    let state = enriched.availability.as_ref().map_or_else(
        || github_status(enriched).and_then(|status| status.issue_state.as_deref()),
        |availability| availability.issue_state.as_deref(),
    );
    state.is_some_and(|state| state.eq_ignore_ascii_case("closed"))
}

fn strong_open_pr_count(enriched: &EnrichedIssue) -> usize {
    if let Some(availability) = &enriched.availability {
        use crate::availability::PullRequestRelation;
        return availability
            .pull_requests
            .iter()
            .filter(|pr| {
                pr.verified
                    && pr.relation == PullRequestRelation::ExplicitResolution
                    && pr.state.as_deref() == Some("open")
                    && pr.merged == Some(false)
            })
            .count();
    }
    enriched
        .competition
        .open_pr_refs
        .max(github_status(enriched).map_or(0, |status| status.linked_open_pr_count))
}

fn github_status(enriched: &EnrichedIssue) -> Option<&super::evidence::GitHubStatusEvidence> {
    enriched
        .decision
        .as_ref()?
        .evidence
        .as_ref()
        .map(|evidence| &evidence.github_status)
}

fn repository_influence(enriched: &EnrichedIssue) -> i32 {
    if (500..1_000).contains(&enriched.repository.stars)
        && (enriched.repository.subscribers.unwrap_or(0) >= 20 || enriched.repository.forks >= 200)
    {
        return 82;
    }
    match enriched.repository.stars {
        10_000.. => 100,
        1_000.. => 82,
        500.. => 68,
        100.. => 45,
        25.. => 22,
        _ => 8,
    }
}

fn repository_low_trust(enriched: &EnrichedIssue) -> bool {
    let repo = &enriched.repository;
    if repo.archived
        || (repo.stars < 100
            && repo.subscribers.unwrap_or(0) == 0
            && repo.forks > repo.stars.saturating_mul(3).max(25))
        || (repo.stars < 500 && repo.forks > repo.stars.max(1).saturating_mul(3))
    {
        return true;
    }
    let age = repo.created_at.as_deref().and_then(|created| {
        let created = DateTime::parse_from_rfc3339(created).ok()?;
        let at = DateTime::parse_from_rfc3339(&enriched.source_fetched_at).ok()?;
        Some((at - created).num_days())
    });
    age.is_some_and(|days| (0..90).contains(&days)) && repo.open_issues.unwrap_or(0) >= 100
}

fn freshness_score(enriched: &EnrichedIssue) -> i32 {
    let (Ok(at), Ok(updated)) = (
        DateTime::parse_from_rfc3339(&enriched.source_fetched_at),
        DateTime::parse_from_rfc3339(&enriched.issue.updated_at),
    ) else {
        return 50;
    };
    match (at - updated).num_days() {
        ..=7 => 100,
        8..=30 => 75,
        31..=90 => 45,
        91..=180 => 25,
        _ => 10,
    }
}

fn band(score: i32) -> ScoreBand {
    match score {
        70.. => ScoreBand::High,
        30.. => ScoreBand::Medium,
        _ => ScoreBand::Low,
    }
}

fn verdict(status: GateStatus, band: GateBand, reason: String) -> GateVerdict {
    GateVerdict::new(status, band, vec![reason], vec!["decision:snapshot".into()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::competition::CompetitionBand;
    use crate::github::GitHubIssue;

    fn issue(body: &str) -> EnrichedIssue {
        let mut enriched = EnrichedIssue::from_issue(&GitHubIssue {
            id: 1,
            number: 1,
            title: "Correct option documentation".into(),
            body: body.into(),
            labels: vec!["good first issue".into()],
            url: "https://github.com/org/project/issues/1".into(),
            repo_full_name: "org/project".into(),
            repo_name: "project".into(),
            repo_description: "Rust command line tool".into(),
            repo_stars: 2000,
            created_at: "2025-01-01T00:00:00Z".into(),
            updated_at: "2025-01-05T00:00:00Z".into(),
        });
        enriched.source_fetched_at = "2025-01-07T00:00:00Z".into();
        enriched
    }

    fn clear() -> SemanticAnswers {
        SemanticAnswers {
            task_type: Some(TaskType::ConcreteChange),
            description_quality: Some(DescriptionQuality::Clear),
            contribution_signal: Some(ContributionSignal::InterestOnly),
            scope: Some(Scope::Bounded),
            preference_match: Some(PreferenceMatch::Matches),
            task_shape: None,
            verification_clues: Some(VerificationClues::Present),
            maintainer_signal: Some(MaintainerSignal::Encouraged),
        }
    }

    #[test]
    fn short_documentation_and_quoted_legacy_words_cannot_reapply_old_penalties() {
        let plain = issue("Replace `--out` with `--output` in the example.");
        let mut poisoned = plain.clone();
        poisoned.issue.body = "No code required is an inaccurate quote. Do not claim; I am NOT working on this. Add docs; grammar; reward pool; dependency dashboard; audit; not that simple.".repeat(100);
        poisoned.competition.claim_comments = 40;
        poisoned.competition.working_comments = 20;
        poisoned.competition.fix_submitted_comments = 30;
        poisoned.competition.competition_points = 100;
        poisoned.competition.competition_band = CompetitionBand::Saturated;
        let baseline = assess(
            &plain,
            Some(&clear()),
            &crate::config::Config::default().profile,
        );
        assert_eq!(
            baseline.recommendation_category,
            RecommendationCategory::HighValueReady
        );
        assert_eq!(
            baseline,
            assess(
                &poisoned,
                Some(&clear()),
                &crate::config::Config::default().profile
            )
        );
        assert_eq!(quality(&poisoned, Some(&clear())).visibility, None);
        assert_eq!(baseline.risk_penalty, 0);
    }

    #[test]
    fn failure_is_unknown_and_does_not_resurrect_keyword_hiding() {
        let enriched =
            issue("No code required. Add trivia. Dependency dashboard. Please assign me.");
        let unknown = assess(&enriched, None, &crate::config::Config::default().profile);
        assert_eq!(
            unknown.recommendation_category,
            RecommendationCategory::NeedsTriage
        );
        assert_eq!(unknown.execution_band, ScoreBand::Medium);
        assert!(unknown.risk_tags.is_empty());
        assert!(unknown
            .missing_evidence
            .iter()
            .any(|item| item.contains("not answered")));
        assert_eq!(quality(&enriched, None).visibility, None);
    }

    #[test]
    fn interest_withdrawal_and_fix_claim_are_distinct_from_actual_open_pr() {
        let mut enriched = issue("A precise correction");
        enriched.competition.closed_pr_refs = 9;
        let mut answers = clear();
        for contribution in [
            ContributionSignal::InterestOnly,
            ContributionSignal::Withdrawn,
        ] {
            answers.contribution_signal = Some(contribution);
            assert_eq!(
                assess(
                    &enriched,
                    Some(&answers),
                    &crate::config::Config::default().profile
                )
                .recommendation_category,
                RecommendationCategory::HighValueReady
            );
            assert_eq!(quality(&enriched, Some(&answers)).visibility, None);
        }
        answers.contribution_signal = Some(ContributionSignal::FixClaimed);
        let claimed = assess(
            &enriched,
            Some(&answers),
            &crate::config::Config::default().profile,
        );
        assert_eq!(
            claimed.recommendation_category,
            RecommendationCategory::HighValueNeedsScoping
        );
        assert!(claimed
            .missing_evidence
            .iter()
            .any(|s| s.contains("claims a fix")));
        assert_eq!(quality(&enriched, Some(&answers)).visibility, None);
        enriched.competition.open_pr_refs = 1;
        answers.contribution_signal = Some(ContributionSignal::Withdrawn);
        answers.task_type = Some(TaskType::SupportQuestion);
        assert_eq!(
            assess(
                &enriched,
                Some(&answers),
                &crate::config::Config::default().profile
            )
            .recommendation_category,
            RecommendationCategory::ContestedOrLowTrust
        );
        assert_eq!(
            quality(&enriched, Some(&answers)).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
    }

    #[test]
    fn facts_override_positive_answers_without_charging_semantics_again() {
        let mut enriched = issue("Concrete correction");
        enriched.repository.archived = true;
        let assessment = assess(
            &enriched,
            Some(&clear()),
            &crate::config::Config::default().profile,
        );
        assert_eq!(assessment.gates.repo_influence.status, GateStatus::HardFail);
        assert_eq!(
            quality(&enriched, Some(&clear())).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
        assert_eq!(quality(&enriched, Some(&clear())).penalty, 0);
    }

    #[test]
    fn uncertain_material_remains_visible_and_frozen_time_replays() {
        let enriched = issue("Concrete correction");
        let mut answers = clear();
        answers.contribution_signal = Some(ContributionSignal::Unknown);
        answers.verification_clues = Some(VerificationClues::Unclear);
        let first = assess(
            &enriched,
            Some(&answers),
            &crate::config::Config::default().profile,
        );
        let saved: EnrichedIssue =
            serde_json::from_str(&serde_json::to_string(&enriched).unwrap()).unwrap();
        assert_eq!(
            first,
            assess(
                &saved,
                Some(&answers),
                &crate::config::Config::default().profile
            )
        );
        assert!(first
            .missing_evidence
            .iter()
            .any(|s| s.contains("do not establish")));
        assert_eq!(quality(&saved, Some(&answers)).visibility, None);
    }

    #[test]
    fn closed_state_and_assignees_survive_positive_semantics_and_failed_provider() {
        let mut enriched = issue("Correct the example");
        let source = GitHubIssue {
            id: 1,
            number: enriched.issue.number,
            title: enriched.issue.title.clone(),
            body: enriched.issue.body.clone(),
            labels: enriched.issue.labels.clone(),
            url: enriched.issue.url.clone(),
            repo_full_name: enriched.issue.repo_full_name.clone(),
            repo_name: enriched.repository.name.clone(),
            repo_description: enriched.repository.description.clone(),
            repo_stars: enriched.repository.stars,
            created_at: enriched.issue.created_at.clone(),
            updated_at: enriched.issue.updated_at.clone(),
        };
        let mut evidence = super::super::evidence::EvidenceSnapshot::from_issue(&source, &enriched);
        evidence.github_status.issue_state = Some("open".into());
        evidence.github_status.assignees = Some(vec!["contributor".into()]);
        evidence.warnings.push("Discussion was sampled".into());
        let mut snapshot = super::super::JudgmentSnapshot::pending("Provider unavailable");
        snapshot.evidence = Some(evidence);
        enriched.decision = Some(snapshot);
        let assigned = assess(
            &enriched,
            Some(&clear()),
            &crate::config::Config::default().profile,
        );
        assert_eq!(
            assigned.recommendation_category,
            RecommendationCategory::ContestedOrLowTrust
        );
        assert!(assigned
            .missing_evidence
            .iter()
            .any(|s| s.contains("Provider unavailable")));
        assert!(assigned
            .missing_evidence
            .iter()
            .any(|s| s.contains("Discussion was sampled")));
        assert_eq!(quality(&enriched, Some(&clear())).visibility, None);
        enriched
            .decision
            .as_mut()
            .unwrap()
            .evidence
            .as_mut()
            .unwrap()
            .github_status
            .issue_state = Some("closed".into());
        assert_eq!(
            quality(&enriched, Some(&clear())).visibility,
            Some(RecommendationVisibility::HiddenDone)
        );
        assert_eq!(
            quality(&enriched, None).visibility,
            Some(RecommendationVisibility::HiddenDone)
        );
    }

    #[test]
    fn numeric_repository_thresholds_are_preserved_with_a_frozen_age() {
        let mut enriched = issue("Concrete correction");
        enriched.repository.stars = 500;
        enriched.repository.subscribers = Some(20);
        assert_eq!(
            assess(
                &enriched,
                Some(&clear()),
                &crate::config::Config::default().profile
            )
            .recommendation_category,
            RecommendationCategory::HighValueReady
        );
        enriched.repository.stars = 30;
        enriched.repository.forks = 500;
        assert_eq!(
            quality(&enriched, None).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
        enriched.repository.stars = 2_000;
        enriched.repository.forks = 0;
        enriched.repository.created_at = Some("2025-01-01T00:00:00Z".into());
        enriched.repository.open_issues = Some(100);
        assert_eq!(
            quality(&enriched, None).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
        enriched.source_fetched_at = "2025-07-01T00:00:00Z".into();
        assert_eq!(quality(&enriched, None).visibility, None);
    }
    #[test]
    fn legacy_task_form_and_discussion_expressions_cannot_establish_unavailability() {
        use crate::decision::questions::TaskShape;
        let enriched = issue("A concrete, bounded repository correction");
        let baseline = assess(
            &enriched,
            Some(&clear()),
            &crate::config::Config::default().profile,
        );
        for shape in [
            TaskShape::ContentFill,
            TaskShape::LearningExercise,
            TaskShape::StatusDashboard,
            TaskShape::GeneratedTemplate,
            TaskShape::ContributionCampaign,
            TaskShape::RewardMarketplace,
        ] {
            let mut answers = clear();
            answers.task_shape = Some(shape);
            assert_eq!(
                assess(
                    &enriched,
                    Some(&answers),
                    &crate::config::Config::default().profile
                ),
                baseline,
                "historical form {shape:?} must not influence current policy"
            );
            assert_eq!(quality(&enriched, Some(&answers)).visibility, None);
        }
        for signal in [
            ContributionSignal::Working,
            ContributionSignal::FixClaimed,
            ContributionSignal::Conflicting,
        ] {
            let mut answers = clear();
            answers.contribution_signal = Some(signal);
            let assessed = assess(
                &enriched,
                Some(&answers),
                &crate::config::Config::default().profile,
            );
            assert_eq!(
                assessed.recommendation_category,
                RecommendationCategory::HighValueNeedsScoping
            );
            assert_eq!(assessed.gates.competition.status, GateStatus::Pass);
            assert!(!assessed.risk_tags.contains(&RiskTag::CompetitionContested));
            assert_eq!(assessed.final_rank_score, baseline.final_rank_score);
            assert_eq!(quality(&enriched, Some(&answers)).visibility, None);
        }
    }

    #[test]
    fn fresh_availability_relations_override_raw_pr_counts_and_unknown_leads_remain_visible() {
        use crate::availability::{
            AvailabilityDepth, AvailabilitySnapshot, CoverageStatus, PullRequestEvidence,
            PullRequestRelation,
        };
        let mut enriched = issue("Correct the --output example");
        enriched.competition.open_pr_refs = 4;
        let mut availability = AvailabilitySnapshot::new(AvailabilityDepth::Final);
        availability.repo_full_name = "org/project".into();
        availability.issue_state = Some("open".into());
        availability.assignees = Some(Vec::new());
        availability.archived = Some(false);
        availability.locked = Some(false);
        availability.default_branch = Some("main".into());
        availability.linked_coverage.status = CoverageStatus::Complete;
        availability.search_coverage.status = CoverageStatus::Complete;
        availability.pull_requests.push(PullRequestEvidence {
            repo_full_name: "org/project".into(),
            number: 23,
            url: None,
            state: Some("open".into()),
            draft: Some(false),
            merged: Some(false),
            merged_at: None,
            base_branch: Some("main".into()),
            relation: PullRequestRelation::Mention,
            sources: vec!["timeline".into()],
            verified: true,
            errors: Vec::new(),
        });
        enriched.availability = Some(availability);
        let evaluate = |enriched: &EnrichedIssue| {
            assess(
                enriched,
                Some(&clear()),
                &crate::config::Config::default().profile,
            )
        };
        assert_eq!(
            evaluate(&enriched).recommendation_category,
            RecommendationCategory::HighValueNeedsScoping,
            "an open PR mention is a lead to investigate, not proof of overlap or availability"
        );
        assert_eq!(quality(&enriched, Some(&clear())).visibility, None);
        enriched.availability.as_mut().unwrap().pull_requests[0].relation =
            PullRequestRelation::SearchLead;
        assert_eq!(
            evaluate(&enriched).recommendation_category,
            RecommendationCategory::HighValueNeedsScoping
        );
        assert_eq!(quality(&enriched, Some(&clear())).visibility, None);
        enriched.availability.as_mut().unwrap().pull_requests[0].relation =
            PullRequestRelation::ExplicitResolution;
        assert_eq!(
            evaluate(&enriched).recommendation_category,
            RecommendationCategory::ContestedOrLowTrust
        );
        assert_eq!(
            quality(&enriched, Some(&clear())).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
        let pr = &mut enriched.availability.as_mut().unwrap().pull_requests[0];
        pr.state = Some("closed".into());
        pr.merged = Some(false);
        assert_eq!(
            evaluate(&enriched).recommendation_category,
            RecommendationCategory::HighValueReady
        );
        assert_eq!(quality(&enriched, Some(&clear())).visibility, None);
        let pr = &mut enriched.availability.as_mut().unwrap().pull_requests[0];
        pr.merged = Some(true);
        pr.merged_at = Some("2025-01-06T00:00:00Z".into());
        assert_eq!(
            quality(&enriched, Some(&clear())).visibility,
            Some(RecommendationVisibility::HiddenQuality)
        );
        enriched.availability.as_mut().unwrap().pull_requests[0].base_branch =
            Some("release".into());
        assert_eq!(
            quality(&enriched, Some(&clear())).visibility,
            None,
            "branch-specific merged evidence is not a verified default-branch resolution"
        );
        assert_eq!(
            evaluate(&enriched).recommendation_category,
            RecommendationCategory::HighValueNeedsScoping,
            "branch-specific fixes need verification before a fresh implementation"
        );
    }
}
