pub use crate::value_model::{
    GateBand, GateStatus, GateVerdict, RankedValueIssue, RecommendationCategory, RiskTag,
    ScoreBand, ValueAssessment, ValueEvidence, ValueGates, ValueScores,
};

use crate::competition::{is_issue_finder_projection_comment, CompetitionBand};
use crate::config::ProfileConfig;
use crate::github_enrichment::EnrichedIssue;
use crate::value_gates::{
    competition_gate, fork_star_ratio, low_depth_gate, marketplace_terms, profile_fit_gate,
    repo_influence_gate,
};
use crate::value_scores::{
    is_low_depth_tag, rank_score, score_band, value_scores, ExecutionQualityAssessment,
    ProfileFitAssessment,
};
use crate::value_signals::{
    build_risk_tags, build_value_signals, risk_penalty, SignalAxis, ValueSignal,
};

pub fn assess_issue(enriched: &EnrichedIssue, profile: &ProfileConfig) -> ValueAssessment {
    let signals = build_value_signals(enriched, profile);
    let mut risk_tags = build_risk_tags(enriched);
    let low_depth = low_depth_gate(enriched);
    let repo_influence = repo_influence_gate(enriched);

    add_gate_risk_tags(enriched, &low_depth, &repo_influence, &mut risk_tags);

    let (scores, fit, execution) =
        value_scores(enriched, profile, &risk_tags, repo_influence.status);
    add_score_risk_tags(&scores, &fit, &execution, &mut risk_tags);

    let competition = competition_gate(&enriched.competition);
    add_competition_risk_tags(enriched, &competition, &mut risk_tags);
    add_scope_risk_tag(enriched, &mut risk_tags);
    sort_dedupe_risk_tags(&mut risk_tags);

    let (scores, fit, execution) =
        value_scores(enriched, profile, &risk_tags, repo_influence.status);
    let profile_fit = profile_fit_gate(&fit);
    let gates = ValueGates {
        low_depth,
        repo_influence,
        competition,
        profile_fit,
    };

    let category = recommendation_category(&gates, &scores, &risk_tags);
    let final_rank_score = rank_score(category, &scores);
    let missing_evidence = missing_evidence(enriched, &gates);
    let evidence = value_evidence(&gates, &fit, &execution, &scores);
    let explanation = top_explanations(&evidence, &signals);

    ValueAssessment {
        final_rank_score,
        category,
        recommendation_category: category,
        gates,
        scores: scores.clone(),
        risk_tags,
        evidence,
        missing_evidence,
        explanation,
        attention_score: scores.repo_influence_score,
        execution_score: scores.execution_quality_score,
        profile_fit_score: scores.profile_fit_score,
        risk_penalty: scores.risk_score,
        attention_band: score_band(scores.repo_influence_score),
        execution_band: score_band(scores.execution_quality_score),
        signals,
    }
}

pub fn aggregate_signals(
    signals: Vec<ValueSignal>,
    risk_tags: Vec<RiskTag>,
    enriched: &EnrichedIssue,
) -> ValueAssessment {
    let attention_score = axis_score(&signals, SignalAxis::Attention);
    let execution_score = axis_score(&signals, SignalAxis::Execution);
    let profile_fit_score = axis_score(&signals, SignalAxis::ProfileFit);
    let risk_penalty = risk_penalty(&risk_tags);
    let attention_band = score_band(attention_score);
    let execution_band = score_band(execution_score);
    let scores = ValueScores {
        repo_influence_score: attention_score,
        profile_fit_score,
        execution_quality_score: execution_score,
        maintainer_signal_score: 0,
        freshness_score: 0,
        risk_score: risk_penalty,
    };
    let category = legacy_recommendation_category(attention_band, execution_band, &risk_tags);
    let final_rank_score = rank_score(category, &scores);
    let explanation = top_signal_explanations(&signals);

    ValueAssessment {
        final_rank_score,
        category,
        recommendation_category: category,
        gates: ValueGates::default(),
        scores,
        risk_tags,
        evidence: Vec::new(),
        missing_evidence: missing_evidence(enriched, &ValueGates::default()),
        explanation,
        attention_score,
        execution_score,
        profile_fit_score,
        risk_penalty,
        attention_band,
        execution_band,
        signals,
    }
}

pub fn final_rank_score(
    attention_score: i32,
    execution_score: i32,
    profile_fit_score: i32,
    risk_penalty: i32,
) -> i32 {
    let scores = ValueScores {
        repo_influence_score: attention_score,
        profile_fit_score,
        execution_quality_score: execution_score,
        maintainer_signal_score: 0,
        freshness_score: 0,
        risk_score: risk_penalty,
    };
    rank_score(RecommendationCategory::HighValueNeedsScoping, &scores)
}

pub fn recommendation_category(
    gates: &ValueGates,
    scores: &ValueScores,
    risk_tags: &[RiskTag],
) -> RecommendationCategory {
    let competition_saturated = gates.competition.band == GateBand::Saturated
        || risk_tags.contains(&RiskTag::CompetitionSaturated);
    let competition_contested = gates.competition.band == GateBand::Contested
        || risk_tags.contains(&RiskTag::CompetitionContested);
    let low_trust = risk_tags.contains(&RiskTag::LowTrustRepo)
        || risk_tags.contains(&RiskTag::MarketplaceNoise)
        || gates.repo_influence.status == GateStatus::HardFail;
    let scope_risk = risk_tags.contains(&RiskTag::ScopeRisk);

    if gates.low_depth.status == GateStatus::HardFail || risk_tags.iter().any(is_low_depth_tag) {
        return RecommendationCategory::FilteredLowDepth;
    }

    if competition_saturated || low_trust {
        return RecommendationCategory::ContestedOrLowTrust;
    }

    if gates.repo_influence.status == GateStatus::Pass
        && gates.competition.status == GateStatus::Pass
        && gates.profile_fit.status == GateStatus::Pass
        && scores.profile_fit_score >= 70
        && scores.execution_quality_score >= 70
        && !scope_risk
    {
        return RecommendationCategory::HighValueReady;
    }

    if gates.repo_influence.status == GateStatus::Pass
        && gates.competition.band != GateBand::Saturated
        && scores.profile_fit_score >= 70
        && scores.execution_quality_score >= 50
        && (!competition_contested || scores.execution_quality_score >= 70)
    {
        return RecommendationCategory::HighValueNeedsScoping;
    }

    if gates.repo_influence.status == GateStatus::SoftFail
        && gates.competition.band != GateBand::Saturated
        && !competition_contested
        && scores.profile_fit_score >= 75
        && scores.execution_quality_score >= 70
    {
        return RecommendationCategory::NicheButActionable;
    }

    if competition_contested {
        return RecommendationCategory::ContestedOrLowTrust;
    }

    RecommendationCategory::NeedsTriage
}

pub fn is_daily_prepare_candidate(assessment: &ValueAssessment) -> bool {
    crate::prepare_gate::default_prepare_allowed(assessment.recommendation_category)
}

fn add_gate_risk_tags(
    enriched: &EnrichedIssue,
    low_depth: &GateVerdict,
    repo_influence: &GateVerdict,
    risk_tags: &mut Vec<RiskTag>,
) {
    if low_depth.status == GateStatus::HardFail && !risk_tags.iter().any(is_low_depth_tag) {
        risk_tags.push(RiskTag::ContentFill);
    }

    if repo_influence.status == GateStatus::HardFail {
        risk_tags.push(RiskTag::LowTrustRepo);
    } else if repo_influence.status == GateStatus::SoftFail {
        risk_tags.push(RiskTag::LowImpactRepo);
    }

    if fork_star_ratio(enriched.repository.stars, enriched.repository.forks) > 3.0
        && enriched.repository.stars < 500
    {
        risk_tags.push(RiskTag::ForkStarAnomaly);
    }
    if !marketplace_terms(enriched).is_empty() {
        risk_tags.push(RiskTag::MarketplaceNoise);
    }
}

fn add_score_risk_tags(
    scores: &ValueScores,
    fit: &ProfileFitAssessment,
    _execution: &ExecutionQualityAssessment,
    risk_tags: &mut Vec<RiskTag>,
) {
    if scores.profile_fit_score < 40 {
        risk_tags.push(RiskTag::ProfileMismatch);
    }
    if fit
        .reasons
        .iter()
        .any(|reason| reason.contains("outside the configured profile"))
    {
        risk_tags.push(RiskTag::ProfileMismatch);
    }
}

fn add_competition_risk_tags(
    enriched: &EnrichedIssue,
    competition: &GateVerdict,
    risk_tags: &mut Vec<RiskTag>,
) {
    if !enriched.competition.warnings.is_empty() {
        risk_tags.push(RiskTag::CompetitionEvidenceMissing);
    }
    match enriched.competition.competition_band {
        CompetitionBand::Contested => risk_tags.push(RiskTag::CompetitionContested),
        CompetitionBand::Saturated => risk_tags.push(RiskTag::CompetitionSaturated),
        CompetitionBand::Clear | CompetitionBand::Light => {}
    }
    if competition.band == GateBand::Contested {
        risk_tags.push(RiskTag::CompetitionContested);
    }
    if competition.band == GateBand::Saturated {
        risk_tags.push(RiskTag::CompetitionSaturated);
    }
}

fn add_scope_risk_tag(enriched: &EnrichedIssue, risk_tags: &mut Vec<RiskTag>) {
    if has_scope_risk(enriched) {
        risk_tags.push(RiskTag::ScopeRisk);
    }
}

fn has_scope_risk(enriched: &EnrichedIssue) -> bool {
    let comment_text = enriched
        .comments
        .iter()
        .filter(|comment| !is_issue_finder_projection_comment(&comment.body_excerpt))
        .map(|comment| comment.body_excerpt.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let text = crate::scoring::normalize(&format!(
        "{} {} {}",
        enriched.issue.title, enriched.issue.body, comment_text
    ));
    text.contains("multiple repositories")
        || text.contains("multiple repos")
        || text.contains("two template")
        || text.contains("template repos")
        || text.contains("template repositories")
        || text.contains("3 fixes required")
        || text.contains("three fixes required")
        || text.contains("briefcase windows visualstudio template")
        || text.contains("briefcase windows app template")
        || text.contains("not that simple")
        || text.contains("don t think it s that simple")
        || text.contains("it s not that simple")
        || text.contains("requires design")
        || text.contains("needs design")
        || text.contains("requires discussion")
        || text.contains("still need to decide")
        || text.contains("semantics unclear")
        || text.contains("semantic discussion")
        || text.contains("is it still valid")
        || text.contains("issue is old")
        || text.contains("would it not be as intuitive")
        || text.contains("is there a reason you are using")
        || text.contains("rare cases")
        || text.matches("github com").count() >= 2
}

fn missing_evidence(enriched: &EnrichedIssue, gates: &ValueGates) -> Vec<String> {
    let mut missing = Vec::new();
    if enriched.growth.recent_stargazer_sample.is_empty() {
        missing.push("Recent stargazer sample was unavailable".to_string());
    }
    if enriched.growth.newest_fork_sample.is_empty() {
        missing.push("Newest fork sample was unavailable".to_string());
    }
    if enriched.comments.is_empty() && enriched.issue.comments_count > 0 {
        missing
            .push("Issue comments count exists but comment excerpts were unavailable".to_string());
    }
    if gates.competition.status == GateStatus::SoftFail
        && gates
            .competition
            .evidence_refs
            .iter()
            .any(|item| item == "issue:timeline")
    {
        missing.push("Competition timeline evidence was unavailable".to_string());
    }
    missing.extend(enriched.warnings.iter().cloned());
    missing.extend(enriched.competition.warnings.iter().cloned());
    missing.sort();
    missing.dedup();
    missing
}

fn value_evidence(
    gates: &ValueGates,
    fit: &ProfileFitAssessment,
    execution: &ExecutionQualityAssessment,
    scores: &ValueScores,
) -> Vec<ValueEvidence> {
    let mut evidence = Vec::new();
    for (name, gate) in [
        ("Low-depth gate", &gates.low_depth),
        ("Repo influence gate", &gates.repo_influence),
        ("Competition gate", &gates.competition),
        ("Profile fit gate", &gates.profile_fit),
    ] {
        evidence.push(ValueEvidence {
            summary: format!(
                "{name}: {} / {} - {}",
                gate.status,
                gate.band,
                gate.reasons.join("; ")
            ),
            evidence_refs: gate.evidence_refs.clone(),
        });
    }
    evidence.push(ValueEvidence {
        summary: format!(
            "Scores: repo {}, profile {}, execution {}, maintainer {}, freshness {}, risk {}",
            scores.repo_influence_score,
            scores.profile_fit_score,
            scores.execution_quality_score,
            scores.maintainer_signal_score,
            scores.freshness_score,
            scores.risk_score
        ),
        evidence_refs: vec!["value_assessment:scores".to_string()],
    });
    evidence.push(ValueEvidence {
        summary: format!("Profile fit: {}", fit.reasons.join("; ")),
        evidence_refs: fit.evidence_refs.clone(),
    });
    evidence.push(ValueEvidence {
        summary: format!("Execution quality: {}", execution.reasons.join("; ")),
        evidence_refs: execution.evidence_refs.clone(),
    });
    evidence
}

fn top_explanations(evidence: &[ValueEvidence], signals: &[ValueSignal]) -> Vec<String> {
    let mut explanations = evidence
        .iter()
        .take(6)
        .map(|item| item.summary.clone())
        .collect::<Vec<_>>();
    for signal in top_signal_explanations(signals) {
        if explanations.len() >= 8 {
            break;
        }
        if !explanations.contains(&signal) {
            explanations.push(signal);
        }
    }
    explanations
}

fn top_signal_explanations(signals: &[ValueSignal]) -> Vec<String> {
    let mut ordered = signals.to_vec();
    ordered.sort_by_key(|signal| std::cmp::Reverse(signal.score_delta));
    ordered
        .into_iter()
        .filter(|signal| signal.score_delta > 0)
        .take(5)
        .map(|signal| signal.summary)
        .collect()
}

fn legacy_recommendation_category(
    attention_band: ScoreBand,
    execution_band: ScoreBand,
    risk_tags: &[RiskTag],
) -> RecommendationCategory {
    if risk_tags.iter().any(is_low_depth_tag) {
        return RecommendationCategory::FilteredLowDepth;
    }
    if attention_band == ScoreBand::High && execution_band == ScoreBand::High {
        return RecommendationCategory::HighValueReady;
    }
    if attention_band == ScoreBand::High {
        return RecommendationCategory::HighValueNeedsScoping;
    }
    if execution_band == ScoreBand::High {
        return RecommendationCategory::NicheButActionable;
    }
    RecommendationCategory::NeedsTriage
}

fn axis_score(signals: &[ValueSignal], axis: SignalAxis) -> i32 {
    signals
        .iter()
        .filter(|signal| signal.axis == axis)
        .map(|signal| signal.score_delta)
        .sum::<i32>()
        .clamp(0, 100)
}

fn sort_dedupe_risk_tags(tags: &mut Vec<RiskTag>) {
    tags.sort_by_key(|tag| tag.to_string());
    tags.dedup();
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{assess_issue, final_rank_score, is_daily_prepare_candidate};
    use crate::competition::{CompetitionBand, CompetitionFacts};
    use crate::config::ProfileConfig;
    use crate::github::GitHubIssue;
    use crate::github_enrichment::{EnrichedComment, EnrichedIssue};
    use crate::value_model::{GateBand, RecommendationCategory, RiskTag};

    fn issue(title: &str, body: &str, stars: u64) -> EnrichedIssue {
        let now = Utc::now().to_rfc3339();
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: title.to_string(),
            body: body.to_string(),
            labels: vec!["good first issue".to_string()],
            url: "https://github.com/owner/repo/issues/1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: "Rust CLI developer tools".to_string(),
            repo_stars: stars,
            created_at: now.clone(),
            updated_at: now,
        };
        let mut enriched = EnrichedIssue::from_issue(&issue);
        enriched.repository.stars = stars;
        enriched.repository.forks = 220;
        enriched.repository.subscribers = Some(40);
        enriched.repository.created_at = Some("2020-01-01T00:00:00Z".to_string());
        enriched.competition = CompetitionFacts::default();
        enriched
    }

    fn profile() -> ProfileConfig {
        ProfileConfig {
            tech_stack: vec!["Rust".to_string()],
            keywords: vec!["cli".to_string()],
        }
    }

    #[test]
    fn classifies_high_value_ready_after_gates() {
        let enriched = issue(
            "Fix Rust CLI parser",
            "Steps to reproduce: run cargo test. Expected graceful behavior, actual panic in src/main.rs. Suggested fix: guard empty input and verify with tests.",
            2_500,
        );
        let assessment = assess_issue(&enriched, &profile());
        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::HighValueReady
        );
        assert!(is_daily_prepare_candidate(&assessment));
    }

    #[test]
    fn comment_scope_pushback_blocks_high_value_ready() {
        let mut enriched = issue(
            "Fix Rust CLI parser",
            "Steps to reproduce: run cargo test. Expected graceful behavior, actual panic in src/main.rs. Suggested fix: guard empty input and verify with tests.",
            2_500,
        );
        enriched.comments.push(EnrichedComment {
            source_ref: "issue:comments.0".to_string(),
            author: Some("maintainer".to_string()),
            author_association: "MEMBER".to_string(),
            created_at: Utc::now().to_rfc3339(),
            body_excerpt:
                "EDIT: I don't think it's that simple because the arguments still need design."
                    .to_string(),
        });

        let assessment = assess_issue(&enriched, &profile());

        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::HighValueNeedsScoping
        );
        assert!(assessment.risk_tags.contains(&RiskTag::ScopeRisk));
    }

    #[test]
    fn saturated_competition_downgrades_to_contested() {
        let mut enriched = issue(
            "Fix Rust CLI parser",
            "Steps to reproduce: run cargo test. Expected graceful behavior, actual panic in src/main.rs. Suggested fix: guard empty input and verify with tests.",
            2_500,
        );
        enriched.competition = CompetitionFacts {
            open_pr_refs: 2,
            closed_pr_refs: 3,
            competition_points: 9,
            competition_band: CompetitionBand::Saturated,
            ..CompetitionFacts::default()
        };
        let assessment = assess_issue(&enriched, &profile());
        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::ContestedOrLowTrust
        );
        assert!(assessment
            .risk_tags
            .contains(&RiskTag::CompetitionSaturated));
        assert_eq!(assessment.gates.competition.band, GateBand::Saturated);
    }

    #[test]
    fn low_depth_is_filtered_before_high_value_gate() {
        let enriched = issue(
            "Add new Grammar Point",
            "No Code Required. This can be done from your browser in under 60 seconds. Add JSON content.",
            2_500,
        );
        let assessment = assess_issue(&enriched, &profile());
        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::FilteredLowDepth
        );
        assert!(!is_daily_prepare_candidate(&assessment));
    }

    #[test]
    fn missing_timeline_blocks_ready() {
        let mut enriched = issue(
            "Fix Rust CLI parser",
            "Steps to reproduce: run cargo test. Expected graceful behavior, actual panic in src/main.rs. Suggested fix: guard empty input and verify with tests.",
            2_500,
        );
        enriched.competition = CompetitionFacts::missing_timeline();
        let assessment = assess_issue(&enriched, &profile());
        assert_eq!(
            assessment.recommendation_category,
            RecommendationCategory::HighValueNeedsScoping
        );
        assert!(assessment
            .risk_tags
            .contains(&RiskTag::CompetitionEvidenceMissing));
    }

    #[test]
    fn compatibility_rank_score_uses_profile_first_rank_axes() {
        assert_eq!(final_rank_score(100, 100, 100, 100), 55);
    }
}
