//! Fixed business questions; providers translate this contract without changing its meaning.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::ProfileConfig;

use super::contract::{
    Answer, AnswerStatus, DecisionRequest, DecisionResponse, Question, QuestionKind,
};
use super::evidence::{ContextCategory, EvidenceSnapshot};

pub const QUESTION_SET_VERSION: &str = "scout-semantics-v2";
pub const HISTORICAL_QUESTION_SET_VERSION: &str = "scout-semantics-v1";

macro_rules! choices {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const OPTIONS: &'static [&'static str] = &[$($value),+];
        }
    };
}

choices!(TaskType {
    ConcreteChange => "concrete_change", SupportQuestion => "support_question",
    OpenDiscussion => "open_discussion", Unclear => "unclear"
});
choices!(DescriptionQuality { Clear => "clear", Partial => "partial", Unclear => "unclear" });
choices!(ContributionSignal {
    InterestOnly => "interest_only", Working => "working", Withdrawn => "withdrawn",
    FixClaimed => "fix_claimed", Conflicting => "conflicting", NotObserved => "not_observed",
    Unknown => "unknown"
});
choices!(Scope { Bounded => "bounded", DesignNeeded => "design_needed", Broad => "broad", Unclear => "unclear" });
choices!(PreferenceMatch { Matches => "matches", Mismatch => "mismatch", NotSpecified => "not_specified", Unclear => "unclear" });
choices!(TaskShape {
    Implementation => "implementation", Documentation => "documentation",
    ContentFill => "content_fill", LearningExercise => "learning_exercise",
    StatusDashboard => "status_dashboard", GeneratedTemplate => "generated_template",
    ContributionCampaign => "contribution_campaign", RewardMarketplace => "reward_marketplace",
    Unclear => "unclear"
});
choices!(VerificationClues { Present => "present", Absent => "absent", Unclear => "unclear" });
choices!(MaintainerSignal {
    Encouraged => "encouraged", ClarificationNeeded => "clarification_needed",
    Deferred => "deferred", NotObserved => "not_observed", Unknown => "unknown"
});

/// Missing answers stay unknown, including explicit unable-to-answer responses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SemanticAnswers {
    pub task_type: Option<TaskType>,
    pub description_quality: Option<DescriptionQuality>,
    pub contribution_signal: Option<ContributionSignal>,
    pub scope: Option<Scope>,
    pub preference_match: Option<PreferenceMatch>,
    /// Retained only to decode historical eight-question judgments. Current screening ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_shape: Option<TaskShape>,
    pub verification_clues: Option<VerificationClues>,
    pub maintainer_signal: Option<MaintainerSignal>,
}

impl SemanticAnswers {
    pub fn from_response(request: &DecisionRequest, response: &DecisionResponse) -> Result<Self> {
        response
            .validate(request)
            .context("Invalid decision model response contract")?;
        if request.candidate_id != response.candidate_id || request.input_id != response.input_id {
            bail!("Decision model response does not identify the requested candidate and material");
        }
        let mut answers = serde_json::Map::new();
        for result in &response.answers {
            let question = request
                .questions
                .iter()
                .find(|q| q.id == result.question_id)
                .context("Decision model response contains an unexpected question")?;
            if answers.contains_key(&result.question_id) {
                bail!("Decision model response contains a duplicate question answer");
            }
            let value = if result.status == AnswerStatus::Answered {
                let Some(Answer::Choice(value)) = &result.answer else {
                    bail!("Business semantic questions require a choice answer");
                };
                let QuestionKind::Choice { options } = &question.kind else {
                    bail!("Business semantic question has an unexpected type");
                };
                if !options.contains(value) {
                    bail!("Decision model response contains an illegal answer choice");
                }
                Value::String(value.clone())
            } else {
                if result.answer.is_some() {
                    bail!("Unanswered decision model question unexpectedly contains an answer");
                }
                Value::Null
            };
            answers.insert(result.question_id.clone(), value);
        }
        if answers.len() != request.questions.len() {
            bail!("Decision model response is missing a question result");
        }
        serde_json::from_value(Value::Object(answers)).context("Invalid business semantic answer")
    }
}

pub fn questions(evidence: &EvidenceSnapshot, profile: &ProfileConfig) -> Vec<Question> {
    let mut questions = historical_questions(evidence, profile);
    questions.retain(|question| question.id != "task_shape");
    let task = questions
        .iter_mut()
        .find(|question| question.id == "task_type")
        .unwrap();
    task.criteria.push("Documentation, content, generated, event-labelled and rewarded tasks qualify when they request a concrete change; evaluate their goal, clarity and scope without treating the task form as exclusion.".into());
    let contribution = questions
        .iter_mut()
        .find(|question| question.id == "contribution_signal")
        .unwrap();
    contribution.prompt = "What contribution expressions appear in the supplied discussion?".into();
    contribution.criteria.push("These are discussion expressions, not issue availability facts. Working, fix_claimed and conflicting require checking current GitHub evidence; they cannot establish an assignment, open PR, closed issue or verified fix.".into());
    questions
}

/// Versioned contracts keep old response validation and input hashes reproducible.
pub fn questions_for_version(
    version: &str,
    evidence: &EvidenceSnapshot,
    profile: &ProfileConfig,
) -> Result<Vec<Question>> {
    match version {
        QUESTION_SET_VERSION => Ok(questions(evidence, profile)),
        HISTORICAL_QUESTION_SET_VERSION => Ok(historical_questions(evidence, profile)),
        _ => bail!("unsupported semantic question set version: {version}"),
    }
}

// Do not edit the v1 contract: historical input identities include every question byte.
fn historical_questions(evidence: &EvidenceSnapshot, profile: &ProfileConfig) -> Vec<Question> {
    let task = evidence.context_for(ContextCategory::Task);
    let contribution = evidence.context_for(ContextCategory::Contribution);
    let mut preference = evidence.context_for(ContextCategory::Preference);
    if let Some(context) = preference.as_object_mut() {
        context.insert(
            "configured_profile".into(),
            serde_json::json!({
                "tech_stack": profile.tech_stack, "keywords": profile.keywords,
            }),
        );
    }
    vec![
        question("task_type", "What kind of task is primarily requested?", TaskType::OPTIONS, task.clone(), &[
            "concrete_change: an identifiable change to repository behavior, tests, documentation or content; a bug report can qualify without a proposed fix.",
            "support_question: primarily asking how to use something; open_discussion: exploring ideas without a settled change; unclear: insufficient material.",
            "Interpret intent, including negation and quotations. Do not classify using word counts or trigger words.",
        ]),
        question("description_quality", "Does the material identify the change goal and how to recognize the expected result?", DescriptionQuality::OPTIONS, task.clone(), &[
            "clear: the goal and recognizable expected result are both specified; partial: only one is clear; unclear: neither is clear.",
            "Short precise corrections may be clear. Long boilerplate, empty templates or repeated prose may be unclear. Test words alone are not verification guidance.",
        ]),
        question("contribution_signal", "What current contribution state is expressed by the supplied discussion?", ContributionSignal::OPTIONS, contribution.clone(), &[
            "interest_only: asking to participate or expressing interest without stating actual work; working: someone explicitly reports current investigation or implementation.",
            "withdrawn: all observed workers have subsequently withdrawn; one person's withdrawal does not cancel another person's work.",
            "fix_claimed: a participant claims a fix, PR submission or resolution; this never establishes a verified fix, an actual open PR, or a merge.",
            "Account for each author, role, chronology, negation and quoted speech. Do not count quoted or denied work as the speaker's work.",
            "conflicting: provided statements cannot be reconciled; not_observed: no contribution expression in available comments; unknown: missing discussion or incomplete chronology prevents determining current status.",
        ]),
        question("scope", "How settled and bounded is the requested change?", Scope::OPTIONS, task.clone(), &[
            "bounded: identifiable scope with no outstanding design decision; design_needed: a decision or maintainer clarification must precede implementation.",
            "broad: an audit, campaign or change across substantial independent components with no bounded unit; unclear: material does not establish scope.",
            "Read relevant clarification comments. Do not infer actual repair difficulty or successful reproduction.",
        ]),
        question("preference_match", "Does this task fit the user's explicit requirements and configured interests?", PreferenceMatch::OPTIONS, preference, &[
            "matches: the requested work fits supplied requirements/profile; mismatch: supplied requirements/profile exclude it or it concerns a different domain.",
            "not_specified: no substantive preference is supplied; unclear: available material cannot resolve fit. Explicit user requirements take precedence over configured interests.",
            "Distinguish repository language from task domain. Documentation work is excluded only when preferences exclude it. Do not invent preferences.",
        ]),
        question("task_shape", "What form does the requested contribution actually take?", TaskShape::OPTIONS, evidence.context_for(ContextCategory::TaskForm), &[
            "implementation: repository behavior, tests or engineering change; documentation: a concrete documentation correction or improvement; content_fill: repetitive addition to a content collection.",
            "learning_exercise: mainly a toy exercise or portfolio practice; status_dashboard: bot-maintained status rather than a requested contribution.",
            "generated_template: generic generated boilerplate without issue-specific work; contribution_campaign: event/campaign participation without an independent bounded engineering goal; reward_marketplace: primarily a claim/reward queue rather than a repository change.",
            "unclear: form is unsupported. Generated or event-labelled issues with concrete engineering goals remain implementation. A bounty alone does not make a marketplace.",
        ]),
        question("verification_clues", "Are concrete clues provided for recognizing or checking the requested result?", VerificationClues::OPTIONS, task, &[
            "present: an observable result, reproduction procedure, relevant test, expected output or exact documentation correction is specified.",
            "absent: complete material offers no concrete check; unclear: missing or truncated material prevents determination. Mentioning tests or coverage in boilerplate is insufficient.",
            "This asks about supplied guidance, not whether a fix has already been verified or tests can actually run.",
        ]),
        question("maintainer_signal", "What relevant position do repository maintainers express about proceeding?", MaintainerSignal::OPTIONS, contribution, &[
            "Use author associations OWNER, MEMBER or COLLABORATOR to identify maintainers, not guesses from writing style. encouraged: they invite or confirm the change; clarification_needed: they require a design/scope decision; deferred: they say hold off, reject, or postpone the change.",
            "not_observed: no relevant maintainer position in supplied comments; unknown: incomplete or conflicting material prevents interpretation. Interest from a contributor is not maintainer approval.",
        ]),
    ]
}

fn question(
    id: &str,
    prompt: &str,
    options: &[&str],
    context: Value,
    criteria: &[&str],
) -> Question {
    Question {
        id: id.to_string(),
        prompt: prompt.to_string(),
        criteria: criteria.iter().map(|s| s.to_string()).collect(),
        context,
        kind: QuestionKind::Choice {
            options: options.iter().map(|s| s.to_string()).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::super::contract::{ProviderMetadata, QuestionResponse, ResponseStatus};
    use super::*;
    use crate::github::GitHubIssue;
    use crate::github_enrichment::EnrichedIssue;

    fn request_and_response() -> (DecisionRequest, DecisionResponse) {
        let issue = GitHubIssue {
            id: 1,
            number: 1,
            title: "Correct --output example".into(),
            body: "Use --output instead of --out.".into(),
            labels: Vec::new(),
            url: "https://github.com/org/project/issues/1".into(),
            repo_full_name: "org/project".into(),
            repo_name: "project".into(),
            repo_description: String::new(),
            repo_stars: 2000,
            created_at: String::new(),
            updated_at: String::new(),
        };
        let evidence = EvidenceSnapshot::from_issue(&issue, &EnrichedIssue::from_issue(&issue));
        let request = DecisionRequest {
            candidate_id: "org/project#1".into(),
            input_id: "frozen-material".into(),
            questions: questions(&evidence, &crate::config::Config::default().profile),
        };
        let response = DecisionResponse {
            candidate_id: request.candidate_id.clone(),
            input_id: request.input_id.clone(),
            status: ResponseStatus::Complete,
            answers: request
                .questions
                .iter()
                .map(|q| {
                    let QuestionKind::Choice { options } = &q.kind else {
                        panic!("Business questions must be finite choices");
                    };
                    QuestionResponse {
                        question_id: q.id.clone(),
                        status: AnswerStatus::Answered,
                        answer: Some(Answer::Choice(options[0].clone())),
                        probabilities: None,
                    }
                })
                .collect(),
            metadata: ProviderMetadata::default(),
        };
        (request, response)
    }

    #[test]
    fn unavailable_contribution_answer_is_unknown_not_false_or_interest() {
        let (request, mut response) = request_and_response();
        let result = response
            .answers
            .iter_mut()
            .find(|r| r.question_id == "contribution_signal")
            .unwrap();
        result.status = AnswerStatus::UnableToAnswer;
        result.answer = None;
        response.status = ResponseStatus::Partial;
        let parsed = SemanticAnswers::from_response(&request, &response).unwrap();
        assert_eq!(parsed.contribution_signal, None);
        assert_eq!(parsed.task_type, Some(TaskType::ConcreteChange));
    }

    #[test]
    fn wrong_candidate_stale_material_duplicate_missing_and_illegal_answers_are_rejected() {
        let (request, valid) = request_and_response();
        assert!(SemanticAnswers::from_response(&request, &valid).is_ok());
        let mut wrong = valid.clone();
        wrong.candidate_id = "org/project#2".into();
        assert!(SemanticAnswers::from_response(&request, &wrong).is_err());
        let mut stale = valid.clone();
        stale.input_id = "old-material".into();
        assert!(SemanticAnswers::from_response(&request, &stale).is_err());
        let mut duplicate = valid.clone();
        duplicate.answers[1] = duplicate.answers[0].clone();
        assert!(SemanticAnswers::from_response(&request, &duplicate).is_err());
        let mut missing = valid.clone();
        missing.answers.pop();
        assert!(SemanticAnswers::from_response(&request, &missing).is_err());
        let mut illegal = valid.clone();
        illegal.answers[0].answer = Some(Answer::Choice("high_quality".into()));
        assert!(SemanticAnswers::from_response(&request, &illegal).is_err());
        let mut wrong_type = valid;
        wrong_type.answers[0].answer = Some(Answer::Boolean(true));
        assert!(SemanticAnswers::from_response(&request, &wrong_type).is_err());
    }
}
