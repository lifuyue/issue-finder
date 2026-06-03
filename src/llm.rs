use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::handoff::{Handoff, LlmEnhancement};

const LLM_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

pub async fn enhance_handoff(config: &Config, handoff: &mut Handoff) {
    if !config.llm.enabled {
        handoff.llm_enhancement = LlmEnhancement::disabled();
        return;
    }

    match request_summary(config, handoff).await {
        Ok(summary) if !summary.trim().is_empty() => {
            handoff.llm_enhancement = LlmEnhancement::success(summary.trim().to_string());
        }
        Ok(_) => {
            handoff.llm_enhancement = LlmEnhancement::failed("LLM returned an empty summary");
        }
        Err(error) => {
            handoff.llm_enhancement = LlmEnhancement::failed(error.to_string());
        }
    }
}

async fn request_summary(config: &Config, handoff: &Handoff) -> Result<String> {
    let api_key = config.resolved_llm_api_key();
    if api_key.trim().is_empty() {
        anyhow::bail!("LLM is enabled but no API key is configured");
    }

    let base_url = config.llm.base_url.trim_end_matches('/');
    let url = format!("{base_url}/chat/completions");
    let client = reqwest::Client::builder()
        .user_agent("patchbay-cli")
        .timeout(LLM_REQUEST_TIMEOUT)
        .build()?;

    let prompt = format!(
        "Summarize this GitHub issue handoff in 3 concise bullets. Do not change instructions, files, commands, branches, or safety constraints.\n\nRepo: {}\nIssue: #{} {}\nBody:\n{}",
        handoff.issue.repo_full_name,
        handoff.issue.number,
        handoff.issue.title,
        handoff.issue.body
    );

    let request = ChatCompletionRequest {
        model: config.llm.model.clone(),
        temperature: 0.2,
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You improve human readability only. You do not make workflow decisions."
                    .to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: prompt,
            },
        ],
    };

    let response = client
        .post(url)
        .bearer_auth(api_key.trim())
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("LLM request failed with {status}: {body}");
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
    use super::enhance_handoff;
    use crate::config::Config;
    use crate::context_pack::default_context_pack;
    use crate::evidence_pack::EvidencePack;
    use crate::handoff::{
        Handoff, HandoffContext, HandoffInstructions, HandoffIssue, HandoffWorkspace,
        LlmEnhancement,
    };
    use crate::llm_review::LlmReview;
    use crate::value_scoring::{RecommendationCategory, ScoreBand, ValueAssessment};

    #[tokio::test]
    async fn llm_disabled_is_non_blocking() {
        let config = Config::default();
        let mut handoff = Handoff {
            version: 1,
            kind: "patchbay_handoff".to_string(),
            id: "id".to_string(),
            created_at: "now".to_string(),
            issue: HandoffIssue {
                repo_full_name: "owner/repo".to_string(),
                number: 1,
                title: "Issue".to_string(),
                body: String::new(),
                labels: vec![],
                url: String::new(),
                updated_at: String::new(),
            },
            workspace: HandoffWorkspace {
                path: String::new(),
                default_branch: "main".to_string(),
                branch: "patchbay/1-issue".to_string(),
                dirty: false,
            },
            context: HandoffContext {
                candidate_files: vec![],
                validation_commands: vec![],
                warnings: vec![],
            },
            context_pack: default_context_pack(),
            value_assessment: ValueAssessment {
                final_rank_score: 0,
                attention_score: 0,
                execution_score: 0,
                profile_fit_score: 0,
                risk_penalty: 0,
                recommendation_category: RecommendationCategory::NeedsTriage,
                attention_band: ScoreBand::Low,
                execution_band: ScoreBand::Low,
                signals: vec![],
                risk_tags: vec![],
                missing_evidence: vec![],
                explanation: vec![],
            },
            evidence_pack: EvidencePack::empty(),
            instructions: HandoffInstructions::default(),
            llm_enhancement: LlmEnhancement::success("old"),
            llm_review: LlmReview::disabled(),
        };

        enhance_handoff(&config, &mut handoff).await;
        assert_eq!(handoff.llm_enhancement.status, "disabled");
    }
}
