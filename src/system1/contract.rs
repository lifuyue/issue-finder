use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequest {
    pub candidate_id: String,
    pub input_id: String,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub criteria: Vec<String>,
    pub context: Value,
    pub kind: QuestionKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuestionKind {
    Boolean,
    Choice { options: Vec<String> },
    Score { levels: Vec<ScoreLevel> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScoreLevel {
    pub id: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Answer {
    Boolean(bool),
    Choice(String),
    Score(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProbabilityInfo {
    Boolean {
        true_probability: f64,
    },
    Choice {
        distribution: Vec<AnswerProbability>,
    },
    Score {
        distribution: Vec<AnswerProbability>,
        expected_value: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnswerProbability {
    pub answer: String,
    pub probability: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnswerStatus {
    Answered,
    UnableToAnswer,
    ProviderFailed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Complete,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct QuestionResponse {
    pub question_id: String,
    pub status: AnswerStatus,
    pub answer: Option<Answer>,
    pub probabilities: Option<ProbabilityInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderMetadata {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: String,
    pub duration_ms: Option<u64>,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DecisionResponse {
    pub candidate_id: String,
    pub input_id: String,
    pub status: ResponseStatus,
    pub answers: Vec<QuestionResponse>,
    pub metadata: ProviderMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error, PartialEq, Eq)]
#[error("{code}: {message}")]
pub struct ProviderError {
    pub code: String,
    pub message: String,
}

impl ProviderError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub type DecisionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<DecisionResponse, ProviderError>> + Send + 'a>>;

/// Providers may return only probabilities obtained from their native service,
/// never confidence percentages generated as answer text.
pub trait Provider: Send + Sync {
    fn fingerprint(&self) -> String;
    fn decide<'a>(&'a self, request: &'a DecisionRequest) -> DecisionFuture<'a>;
}

fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::new("invalid_contract", message)
}

fn distinct_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<(), ProviderError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if id.trim().is_empty() || !seen.insert(id) {
            return Err(invalid(
                "IDs and allowed answers must be nonempty and unique",
            ));
        }
    }
    Ok(())
}

impl DecisionRequest {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.candidate_id.trim().is_empty()
            || self.input_id.trim().is_empty()
            || self.questions.is_empty()
        {
            return Err(invalid("candidate_id, input_id and questions are required"));
        }
        distinct_ids(self.questions.iter().map(|question| question.id.as_str()))?;
        for question in &self.questions {
            if question.prompt.trim().is_empty()
                || question.criteria.is_empty()
                || question
                    .criteria
                    .iter()
                    .any(|criterion| criterion.trim().is_empty())
            {
                return Err(invalid(format!(
                    "question {} requires a prompt and explicit criteria",
                    question.id
                )));
            }
            match &question.kind {
                QuestionKind::Boolean => {}
                QuestionKind::Choice { options } => {
                    if options.len() < 2 {
                        return Err(invalid("choice requires at least two options"));
                    }
                    distinct_ids(options.iter().map(String::as_str))?;
                }
                QuestionKind::Score { levels } => {
                    if levels.len() < 2 {
                        return Err(invalid("score requires at least two ordered levels"));
                    }
                    distinct_ids(levels.iter().map(|level| level.id.as_str()))?;
                    if levels.iter().any(|level| !level.value.is_finite())
                        || levels.windows(2).any(|pair| pair[0].value >= pair[1].value)
                    {
                        return Err(invalid(
                            "score values must be finite and strictly increasing",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl DecisionResponse {
    pub fn validate(&self, request: &DecisionRequest) -> Result<(), ProviderError> {
        request.validate()?;
        if self.candidate_id != request.candidate_id || self.input_id != request.input_id {
            return Err(invalid(
                "response candidate/input identity does not match request",
            ));
        }
        distinct_ids(
            self.answers
                .iter()
                .map(|answer| answer.question_id.as_str()),
        )?;
        if self.answers.len() != request.questions.len() {
            return Err(invalid(
                "response must answer each requested question exactly once",
            ));
        }
        for response in &self.answers {
            let question = request
                .questions
                .iter()
                .find(|question| question.id == response.question_id)
                .ok_or_else(|| invalid("response contains an unknown question ID"))?;
            match response.status {
                AnswerStatus::Answered => {
                    match (&question.kind, &response.answer) {
                        (QuestionKind::Boolean, Some(Answer::Boolean(_))) => {}
                        (QuestionKind::Choice { options }, Some(Answer::Choice(answer)))
                            if options.contains(answer) => {}
                        (QuestionKind::Score { levels }, Some(Answer::Score(answer)))
                            if levels.iter().any(|level| &level.id == answer) => {}
                        _ => {
                            return Err(invalid(format!(
                                "invalid answer type/value for {}",
                                question.id
                            )))
                        }
                    }
                    validate_probabilities(question, response.probabilities.as_ref())?;
                }
                AnswerStatus::UnableToAnswer | AnswerStatus::ProviderFailed => {
                    if response.answer.is_some() || response.probabilities.is_some() {
                        return Err(invalid(
                            "unanswered questions cannot carry answers or probabilities",
                        ));
                    }
                }
            }
        }
        let expected = if self
            .answers
            .iter()
            .all(|answer| answer.status == AnswerStatus::ProviderFailed)
        {
            ResponseStatus::Failed
        } else if self
            .answers
            .iter()
            .all(|answer| answer.status == AnswerStatus::Answered)
        {
            ResponseStatus::Complete
        } else {
            ResponseStatus::Partial
        };
        if self.status != expected {
            return Err(invalid("response status disagrees with question statuses"));
        }
        Ok(())
    }
}

fn valid_probability(value: f64) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

fn distribution_valid(
    distribution: &[AnswerProbability],
    options: &[String],
) -> Result<(), ProviderError> {
    distinct_ids(distribution.iter().map(|entry| entry.answer.as_str()))?;
    if distribution.len() != options.len()
        || distribution
            .iter()
            .any(|entry| !options.contains(&entry.answer) || !valid_probability(entry.probability))
        || (distribution
            .iter()
            .map(|entry| entry.probability)
            .sum::<f64>()
            - 1.0)
            .abs()
            > 1e-6
    {
        return Err(invalid("probability distribution must cover precisely the mutually exclusive answers and sum to one"));
    }
    Ok(())
}

fn validate_probabilities(
    question: &Question,
    probabilities: Option<&ProbabilityInfo>,
) -> Result<(), ProviderError> {
    match (&question.kind, probabilities) {
        (_, None) => Ok(()),
        (QuestionKind::Boolean, Some(ProbabilityInfo::Boolean { true_probability }))
            if valid_probability(*true_probability) =>
        {
            Ok(())
        }
        (QuestionKind::Choice { options }, Some(ProbabilityInfo::Choice { distribution })) => {
            distribution_valid(distribution, options)
        }
        (
            QuestionKind::Score { levels },
            Some(ProbabilityInfo::Score {
                distribution,
                expected_value,
            }),
        ) => {
            distribution_valid(
                distribution,
                &levels
                    .iter()
                    .map(|level| level.id.clone())
                    .collect::<Vec<_>>(),
            )?;
            let expected = distribution
                .iter()
                .map(|entry| {
                    levels
                        .iter()
                        .find(|level| level.id == entry.answer)
                        .unwrap()
                        .value
                        * entry.probability
                })
                .sum::<f64>();
            if !expected_value.is_finite() || (expected - expected_value).abs() > 1e-6 {
                return Err(invalid(
                    "score expected_value disagrees with the native distribution",
                ));
            }
            Ok(())
        }
        _ => Err(invalid("probability type/value does not match question")),
    }
}
