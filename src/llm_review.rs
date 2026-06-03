use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::evidence_pack::EvidencePack;
use crate::github::GitHubIssue;
use crate::value_scoring::ValueAssessment;

const LLM_REVIEW_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmReview {
    pub status: String,
    pub review_summary: Option<String>,
    pub fact_check_notes: Vec<String>,
    pub possible_overclaims: Vec<String>,
    pub agent_brief: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

impl LlmReview {
    pub fn disabled() -> Self {
        Self {
            status: "disabled".to_string(),
            review_summary: None,
            fact_check_notes: Vec::new(),
            possible_overclaims: Vec::new(),
            agent_brief: None,
            warnings: Vec::new(),
        }
    }

    pub fn failed(warning: impl Into<String>) -> Self {
        Self {
            status: "failed".to_string(),
            review_summary: None,
            fact_check_notes: Vec::new(),
            possible_overclaims: Vec::new(),
            agent_brief: None,
            warnings: vec![warning.into()],
        }
    }
}

pub async fn review_handoff(
    config: &Config,
    issue: &GitHubIssue,
    assessment: &ValueAssessment,
    evidence_pack: &EvidencePack,
) -> LlmReview {
    if !config.llm.enabled {
        return LlmReview::disabled();
    }

    let before_score = assessment.final_rank_score;
    let before_category = assessment.recommendation_category;
    let result = request_review(config, issue, assessment, evidence_pack).await;
    debug_assert_eq!(before_score, assessment.final_rank_score);
    debug_assert_eq!(before_category, assessment.recommendation_category);

    match result {
        Ok(text) if !text.trim().is_empty() => LlmReview {
            status: "success".to_string(),
            review_summary: Some(text.trim().to_string()),
            fact_check_notes: Vec::new(),
            possible_overclaims: if evidence_pack.source_refs.is_empty() {
                vec![
                    "LLM review could not cite source_refs because none were available".to_string(),
                ]
            } else {
                Vec::new()
            },
            agent_brief: Some(text.trim().to_string()),
            warnings: Vec::new(),
        },
        Ok(_) => LlmReview::failed("LLM review returned an empty response"),
        Err(error) => LlmReview::failed(error.to_string()),
    }
}

async fn request_review(
    config: &Config,
    issue: &GitHubIssue,
    assessment: &ValueAssessment,
    evidence_pack: &EvidencePack,
) -> Result<String> {
    let api_key = config.resolved_llm_api_key();
    if api_key.trim().is_empty() {
        anyhow::bail!("LLM is enabled but no API key is configured");
    }

    let base_url = config.llm.base_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .user_agent("patchbay-cli")
        .timeout(LLM_REVIEW_TIMEOUT)
        .build()?;
    let prompt = format!(
        "Review this Patchbay evidence package for display only. Do not change scores or recommendation categories.\nRepo issue: {}#{}\nTitle: {}\nFinal rank score: {}\nCategory: {}\nAttention score: {}\nExecution score: {}\nRisk penalty: {}\nSource refs: {}\nEvidence JSON: {}",
        issue.repo_full_name,
        issue.number,
        issue.title,
        assessment.final_rank_score,
        assessment.recommendation_category,
        assessment.attention_score,
        assessment.execution_score,
        assessment.risk_penalty,
        evidence_pack.source_refs.join(", "),
        serde_json::to_string(evidence_pack)?
    );
    let request = ChatCompletionRequest {
        model: config.llm.model.clone(),
        temperature: 0.2,
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You review evidence for presentation only. You cannot modify deterministic scores, recommendations, or selection decisions.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ],
    };
    let response = client
        .post(format!("{base_url}/chat/completions"))
        .bearer_auth(api_key.trim())
        .json(&request)
        .send()
        .await?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("LLM review failed with {status}: {body}");
    }
    let response = response.json::<ChatCompletionResponse>().await?;
    Ok(response
        .choices
        .first()
        .map(|choice| choice.message.content.clone())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::evidence_pack::EvidencePack;
    use crate::github::GitHubIssue;
    use crate::llm_review::review_handoff;
    use crate::value_scoring::{RecommendationCategory, ScoreBand, ValueAssessment};

    #[tokio::test]
    async fn llm_review_cannot_affect_score_or_recommendation_when_disabled() {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: "Issue".to_string(),
            body: String::new(),
            labels: vec![],
            url: String::new(),
            repo_full_name: "owner/repo".to_string(),
            repo_name: "repo".to_string(),
            repo_description: String::new(),
            repo_stars: 0,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let assessment = ValueAssessment {
            final_rank_score: 80,
            attention_score: 90,
            execution_score: 70,
            profile_fit_score: 50,
            risk_penalty: 10,
            recommendation_category: RecommendationCategory::AgentReadyHighValue,
            attention_band: ScoreBand::High,
            execution_band: ScoreBand::High,
            signals: Vec::new(),
            risk_tags: Vec::new(),
            missing_evidence: Vec::new(),
            explanation: Vec::new(),
        };
        let before_score = assessment.final_rank_score;
        let before_category = assessment.recommendation_category;
        let review = review_handoff(
            &Config::default(),
            &issue,
            &assessment,
            &EvidencePack::empty(),
        )
        .await;
        assert_eq!(review.status, "disabled");
        assert_eq!(assessment.final_rank_score, before_score);
        assert_eq!(assessment.recommendation_category, before_category);
    }
}
