use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::evidence_pack::EvidencePack;
use crate::github::GitHubIssue;
use crate::value_scoring::ValueAssessment;

const LLM_REVIEW_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmConfirmation {
    pub status: String,
    pub decision: String,
    pub confidence: Option<f32>,
    pub summary: Option<String>,
    pub fit_notes: Vec<String>,
    pub risk_flags: Vec<String>,
    pub missing_context: Vec<String>,
    pub source_refs_used: Vec<String>,
    pub agent_brief: Option<String>,
    pub warnings: Vec<String>,
}

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

impl LlmConfirmation {
    pub fn disabled() -> Self {
        Self {
            status: "disabled".to_string(),
            decision: "not_run".to_string(),
            confidence: None,
            summary: None,
            fit_notes: Vec::new(),
            risk_flags: Vec::new(),
            missing_context: Vec::new(),
            source_refs_used: Vec::new(),
            agent_brief: None,
            warnings: Vec::new(),
        }
    }

    pub fn failed(warning: impl Into<String>) -> Self {
        Self {
            status: "failed".to_string(),
            decision: "unknown".to_string(),
            confidence: None,
            summary: None,
            fit_notes: Vec::new(),
            risk_flags: Vec::new(),
            missing_context: Vec::new(),
            source_refs_used: Vec::new(),
            agent_brief: None,
            warnings: vec![warning.into()],
        }
    }

    pub fn legacy_review(&self) -> LlmReview {
        match self.status.as_str() {
            "disabled" => LlmReview::disabled(),
            "failed" => LlmReview::failed(
                self.warnings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "LLM confirmation failed".to_string()),
            ),
            _ => LlmReview {
                status: self.status.clone(),
                review_summary: self.summary.clone(),
                fact_check_notes: self.fit_notes.clone(),
                possible_overclaims: self.risk_flags.clone(),
                agent_brief: self.agent_brief.clone(),
                warnings: self.warnings.clone(),
            },
        }
    }

    fn from_response_text(text: &str, source_refs: &[String]) -> Self {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Self::failed("LLM confirmation returned an empty response");
        }

        let parsed = match serde_json::from_str::<LlmConfirmationResponse>(trimmed) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Self::failed(format!("LLM confirmation returned invalid JSON: {error}"))
            }
        };
        let mut warnings = parsed.warnings;
        if source_refs.is_empty() {
            warnings.push(
                "LLM confirmation could not cite source_refs because none were available"
                    .to_string(),
            );
        }

        Self {
            status: "success".to_string(),
            decision: normalized_or(parsed.decision, "needs_human_review"),
            confidence: parsed.confidence,
            summary: normalized_optional(parsed.summary),
            fit_notes: parsed.fit_notes,
            risk_flags: parsed.risk_flags,
            missing_context: parsed.missing_context,
            source_refs_used: parsed.source_refs_used,
            agent_brief: normalized_optional(parsed.agent_brief),
            warnings,
        }
    }
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

impl Default for LlmConfirmation {
    fn default() -> Self {
        Self::disabled()
    }
}

impl Default for LlmReview {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct LlmConfirmationResponse {
    decision: String,
    confidence: Option<f32>,
    summary: Option<String>,
    fit_notes: Vec<String>,
    risk_flags: Vec<String>,
    missing_context: Vec<String>,
    source_refs_used: Vec<String>,
    agent_brief: Option<String>,
    warnings: Vec<String>,
}

impl Default for LlmConfirmationResponse {
    fn default() -> Self {
        Self {
            decision: "needs_human_review".to_string(),
            confidence: None,
            summary: None,
            fit_notes: Vec::new(),
            risk_flags: Vec::new(),
            missing_context: Vec::new(),
            source_refs_used: Vec::new(),
            agent_brief: None,
            warnings: Vec::new(),
        }
    }
}

pub async fn review_handoff(
    config: &Config,
    issue: &GitHubIssue,
    assessment: &ValueAssessment,
    evidence_pack: &EvidencePack,
) -> LlmConfirmation {
    if !config.llm.enabled {
        return LlmConfirmation::disabled();
    }

    let before_score = assessment.final_rank_score;
    let before_category = assessment.recommendation_category;
    let result = request_review(config, issue, assessment, evidence_pack).await;
    debug_assert_eq!(before_score, assessment.final_rank_score);
    debug_assert_eq!(before_category, assessment.recommendation_category);

    match result {
        Ok(text) => LlmConfirmation::from_response_text(&text, &evidence_pack.source_refs),
        Err(error) => LlmConfirmation::failed(error.to_string()),
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
        .user_agent("issue-finder")
        .timeout(LLM_REVIEW_TIMEOUT)
        .build()?;
    let prompt = format!(
        "Review this Issue Finder evidence package for human confirmation only. Do not change scores or recommendation categories. Return exactly one JSON object with fields: decision, confidence, summary, fit_notes, risk_flags, missing_context, source_refs_used, agent_brief, warnings.\nRepo issue: {}#{}\nTitle: {}\nFinal rank score: {}\nCategory: {}\nAttention score: {}\nExecution score: {}\nRisk penalty: {}\nSource refs: {}\nEvidence JSON: {}",
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
                content: "You produce structured confirmation evidence for human review. You cannot modify deterministic scores, recommendations, prepare gates, or selection decisions. Return valid JSON only.".to_string(),
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

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalized_or(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::evidence_pack::EvidencePack;
    use crate::github::GitHubIssue;
    use crate::llm_review::{review_handoff, LlmConfirmation};
    use crate::value_scoring::{RecommendationCategory, ScoreBand, ValueAssessment};

    #[tokio::test]
    async fn llm_confirmation_cannot_affect_score_or_recommendation_when_disabled() {
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
            category: RecommendationCategory::HighValueReady,
            attention_score: 90,
            execution_score: 70,
            profile_fit_score: 50,
            risk_penalty: 10,
            recommendation_category: RecommendationCategory::HighValueReady,
            attention_band: ScoreBand::High,
            execution_band: ScoreBand::High,
            signals: Vec::new(),
            risk_tags: Vec::new(),
            missing_evidence: Vec::new(),
            explanation: Vec::new(),
            ..ValueAssessment::default()
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

    #[test]
    fn parses_structured_llm_confirmation() {
        let response = r#"{
            "decision": "confirm",
            "confidence": 0.82,
            "summary": "Good fit",
            "fit_notes": ["small scope"],
            "risk_flags": ["needs tests"],
            "missing_context": ["comments"],
            "source_refs_used": ["issue.body"],
            "agent_brief": "Start in src/lib.rs",
            "warnings": []
        }"#;

        let confirmation =
            LlmConfirmation::from_response_text(response, &["issue.body".to_string()]);

        assert_eq!(confirmation.status, "success");
        assert_eq!(confirmation.decision, "confirm");
        assert_eq!(confirmation.summary.as_deref(), Some("Good fit"));
        assert_eq!(confirmation.risk_flags, vec!["needs tests"]);
        assert_eq!(
            confirmation.agent_brief.as_deref(),
            Some("Start in src/lib.rs")
        );
    }

    #[test]
    fn malformed_llm_confirmation_is_non_blocking_failure() {
        let confirmation = LlmConfirmation::from_response_text("not json", &[]);

        assert_eq!(confirmation.status, "failed");
        assert_eq!(confirmation.decision, "unknown");
        assert!(confirmation.warnings[0].contains("invalid JSON"));
    }
}
