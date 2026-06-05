use std::collections::HashMap;

use crate::value_scoring::{RankedValueIssue, RecommendationCategory};

use super::events::IssueKey;
use super::feedback::assess_feedback;
use super::freshness::assess_freshness;
use super::model::{category_anchor, RecommendationAssessment, RecommendationVisibility};
use super::state::RecommendationIssueState;

pub fn apply_recommendation_assessments(
    ranked: &mut [RankedValueIssue],
    states: &HashMap<IssueKey, RecommendationIssueState>,
) {
    for item in ranked {
        let issue_key = IssueKey::from_issue(&item.issue);
        let state = states.get(&issue_key);
        item.recommendation = recommendation_assessment(item, state);
        item.score = item.recommendation.final_feed_score;
        item.explanation = recommendation_explanation(item);
    }
}

pub fn sort_by_feed(ranked: &mut [RankedValueIssue]) {
    ranked.sort_by(|left, right| {
        visibility_rank(left.recommendation.visibility)
            .cmp(&visibility_rank(right.recommendation.visibility))
            .then_with(|| {
                right
                    .recommendation
                    .final_feed_score
                    .cmp(&left.recommendation.final_feed_score)
            })
            .then_with(|| {
                left.recommendation
                    .base_category
                    .sort_rank()
                    .cmp(&right.recommendation.base_category.sort_rank())
            })
            .then_with(|| {
                right
                    .recommendation
                    .base_rank_score
                    .cmp(&left.recommendation.base_rank_score)
            })
            .then_with(|| right.issue.updated_at.cmp(&left.issue.updated_at))
    });
}

pub fn recommendation_assessment(
    item: &RankedValueIssue,
    state: Option<&RecommendationIssueState>,
) -> RecommendationAssessment {
    let value = &item.value_assessment;
    let freshness = assess_freshness(&item.enriched_issue);
    let feedback = assess_feedback(state, &item.enriched_issue);
    let mut visibility = feedback.visibility;
    if visibility == RecommendationVisibility::Visible
        && value.recommendation_category == RecommendationCategory::FilteredLowDepth
    {
        visibility = RecommendationVisibility::HiddenFiltered;
    }

    let base_category = value.recommendation_category;
    let base_rank_score = value.final_rank_score;
    let final_feed_score = category_anchor(base_category)
        + base_rank_score
        + freshness.boost
        + feedback.reactivation_boost
        - feedback.penalty;
    let mut reasons = Vec::new();
    reasons.push(format!(
        "Base category {base_category} anchors feed score at {}",
        category_anchor(base_category)
    ));
    reasons.push(format!(
        "Intrinsic value rank contributes +{base_rank_score}"
    ));
    reasons.extend(freshness.reasons);
    reasons.extend(feedback.reasons);
    if visibility != RecommendationVisibility::Visible {
        reasons.push(format!("Feed visibility is {visibility}"));
    }

    RecommendationAssessment {
        base_category,
        base_rank_score,
        freshness_boost: freshness.boost,
        feedback_penalty: feedback.penalty,
        reactivation_boost: feedback.reactivation_boost,
        final_feed_score,
        visibility,
        reasons,
    }
}

pub fn displayable(item: &RankedValueIssue, include_filtered: bool) -> bool {
    item.recommendation.displayable(include_filtered)
}

fn recommendation_explanation(item: &RankedValueIssue) -> Vec<String> {
    let mut explanation = item.value_assessment.explanation.clone();
    explanation.extend(item.recommendation.reasons.clone());
    explanation
}

fn visibility_rank(visibility: RecommendationVisibility) -> u8 {
    match visibility {
        RecommendationVisibility::Visible => 0,
        RecommendationVisibility::HiddenFiltered => 1,
        RecommendationVisibility::HiddenDone => 2,
        RecommendationVisibility::HiddenDismissed => 3,
    }
}
