use serde::{Deserialize, Serialize};
use std::fmt;

use crate::value_scoring::{RecommendationCategory, ValueAssessment};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecommendationAssessment {
    pub base_category: RecommendationCategory,
    pub base_rank_score: i32,
    pub freshness_boost: i32,
    pub feedback_penalty: i32,
    pub reactivation_boost: i32,
    pub final_feed_score: i32,
    pub visibility: RecommendationVisibility,
    pub reasons: Vec<String>,
}

impl RecommendationAssessment {
    pub fn from_value_assessment(value: &ValueAssessment) -> Self {
        let visibility =
            if value.recommendation_category == RecommendationCategory::FilteredLowDepth {
                RecommendationVisibility::HiddenFiltered
            } else {
                RecommendationVisibility::Visible
            };
        let base_rank_score = value.final_rank_score;
        let final_feed_score = category_anchor(value.recommendation_category) + base_rank_score;
        Self {
            base_category: value.recommendation_category,
            base_rank_score,
            freshness_boost: 0,
            feedback_penalty: 0,
            reactivation_boost: 0,
            final_feed_score,
            visibility,
            reasons: Vec::new(),
        }
    }

    pub fn displayable(&self, include_filtered: bool) -> bool {
        match self.visibility {
            RecommendationVisibility::Visible => true,
            RecommendationVisibility::HiddenFiltered => include_filtered,
            RecommendationVisibility::HiddenDone | RecommendationVisibility::HiddenDismissed => {
                false
            }
        }
    }
}

impl Default for RecommendationAssessment {
    fn default() -> Self {
        Self::from_value_assessment(&ValueAssessment::default())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationVisibility {
    #[default]
    Visible,
    HiddenDone,
    HiddenDismissed,
    HiddenFiltered,
}

impl fmt::Display for RecommendationVisibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Visible => "visible",
            Self::HiddenDone => "hidden_done",
            Self::HiddenDismissed => "hidden_dismissed",
            Self::HiddenFiltered => "hidden_filtered",
        })
    }
}

pub fn category_anchor(category: RecommendationCategory) -> i32 {
    match category {
        RecommendationCategory::HighValueReady => 500,
        RecommendationCategory::HighValueNeedsScoping => 470,
        RecommendationCategory::NicheButActionable => 390,
        RecommendationCategory::NeedsTriage => 300,
        RecommendationCategory::ContestedOrLowTrust => 180,
        RecommendationCategory::FilteredLowDepth => 0,
    }
}
