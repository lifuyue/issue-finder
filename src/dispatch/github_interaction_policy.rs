use serde_json::{json, Value};

use super::model::{
    DispatchOutcomeFailureClass, DispatchOutcomeKind, DispatchRun, DispatchRunOutcome,
    DispatchRunStatus, DispatchValidationOutcome, GitHubInteraction, GitHubInteractionDecisionKind,
    GitHubInteractionStatus, GitHubInteractionType, IssueTask,
};

#[derive(Debug, Clone, PartialEq)]
pub struct GitHubInteractionDecisionPlan {
    pub decision_kind: GitHubInteractionDecisionKind,
    pub interaction_type: Option<GitHubInteractionType>,
    pub body: Option<String>,
    pub reason_code: String,
    pub reasons: Vec<String>,
    pub inputs_json: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FinalCommentFacts {
    pub result_artifact_id: Option<String>,
    pub suggested_reply: Option<String>,
    pub result_status: Option<String>,
    pub outcome_kind: Option<DispatchOutcomeKind>,
    pub failure_class: Option<DispatchOutcomeFailureClass>,
    pub validation_outcome: Option<DispatchValidationOutcome>,
}

pub fn decide_tracking_comment(
    issue_task: &IssueTask,
    existing_interactions: &[GitHubInteraction],
    body_override: Option<String>,
) -> GitHubInteractionDecisionPlan {
    let inputs_json = json!({
        "phase": "tracking",
        "issueKey": issue_task.issue_key,
        "issueTaskStatus": issue_task.status.as_str(),
        "bodyOverride": body_override.as_ref().is_some_and(|body| !body.trim().is_empty()),
        "existingInteractions": interaction_inputs(existing_interactions)
    });
    if has_active_interaction(
        existing_interactions,
        GitHubInteractionType::TrackingComment,
    ) {
        return no_comment(
            "duplicate_tracking_interaction",
            vec!["A tracking comment already exists or is awaiting approval for this issue."],
            inputs_json,
        );
    }

    if let Some(body) = normalized_body(body_override) {
        return draft(
            GitHubInteractionDecisionKind::Tracking,
            GitHubInteractionType::TrackingComment,
            body,
            "explicit_tracking_body",
            vec!["An explicit tracking comment body was supplied by the operator."],
            inputs_json,
        );
    }

    no_comment(
        "tracking_default_silence",
        vec!["Tracking comments are silent by default unless the operator supplies a clear public-value body."],
        inputs_json,
    )
}

pub fn decide_final_comment(
    issue_task: &IssueTask,
    run: &DispatchRun,
    outcome: Option<&DispatchRunOutcome>,
    facts: FinalCommentFacts,
    existing_interactions: &[GitHubInteraction],
    body_override: Option<String>,
) -> GitHubInteractionDecisionPlan {
    let context_gap = has_context_gap(run, outcome, &facts);
    let target_interaction = if context_gap {
        GitHubInteractionType::ClarificationComment
    } else {
        GitHubInteractionType::FinalComment
    };
    let inputs_json = json!({
        "phase": "final",
        "issueKey": issue_task.issue_key,
        "runId": run.id,
        "runStatus": run.status.as_str(),
        "outcomeKind": facts.outcome_kind.map(|value| value.as_str()),
        "failureClass": facts.failure_class.map(|value| value.as_str()),
        "validationOutcome": facts.validation_outcome.map(|value| value.as_str()),
        "resultArtifactId": facts.result_artifact_id,
        "resultStatus": facts.result_status,
        "hasSuggestedGitHubReply": facts.suggested_reply.as_ref().is_some_and(|body| !body.trim().is_empty()),
        "bodyOverride": body_override.as_ref().is_some_and(|body| !body.trim().is_empty()),
        "contextGap": context_gap,
        "existingInteractions": interaction_inputs(existing_interactions)
    });

    if has_active_interaction(existing_interactions, target_interaction) {
        return no_comment(
            "duplicate_comment_interaction",
            vec!["A matching GitHub comment interaction already exists or is awaiting approval for this issue."],
            inputs_json,
        );
    }

    if let Some(body) = normalized_body(body_override) {
        let kind = if context_gap {
            GitHubInteractionDecisionKind::Clarification
        } else {
            GitHubInteractionDecisionKind::Final
        };
        return draft(
            kind,
            target_interaction,
            body,
            "explicit_final_body",
            vec!["An explicit GitHub comment body was supplied by the operator."],
            inputs_json,
        );
    }

    if context_gap {
        return match normalized_body(facts.suggested_reply) {
            Some(body) => draft(
                GitHubInteractionDecisionKind::Clarification,
                GitHubInteractionType::ClarificationComment,
                body,
                "context_gap_suggested_reply",
                vec!["The dispatch outcome needs user or maintainer input and supplied a specific suggested GitHub reply."],
                inputs_json,
            ),
            None => no_reply(
                "context_gap_without_suggested_reply",
                vec!["The dispatch outcome needs user or maintainer input, but did not supply a specific suggested GitHub reply."],
                inputs_json,
            ),
        };
    }

    if facts.validation_outcome == Some(DispatchValidationOutcome::Failed) {
        return no_reply(
            "validation_failed",
            vec!["The dispatch outcome recorded failed validation, so Issue Finder will not draft a public final reply."],
            inputs_json,
        );
    }

    match facts.outcome_kind {
        Some(DispatchOutcomeKind::FixReady) | Some(DispatchOutcomeKind::CompletedNoChange) => {
            match normalized_body(facts.suggested_reply) {
                Some(body) => draft(
                    GitHubInteractionDecisionKind::Final,
                    GitHubInteractionType::FinalComment,
                    body,
                    "final_suggested_reply",
                    vec!["The dispatch outcome supplied an explicit suggested GitHub reply with clear public value."],
                    inputs_json,
                ),
                None => no_reply(
                    "missing_suggested_github_reply",
                    vec!["Final comments require an explicit suggestedGitHubReply; generic completion summaries are kept local."],
                    inputs_json,
                ),
            }
        }
        Some(DispatchOutcomeKind::Blocked)
        | Some(DispatchOutcomeKind::Failed)
        | Some(DispatchOutcomeKind::Canceled) => no_reply(
            "terminal_outcome_not_commentable",
            vec!["The dispatch outcome is terminal but not publicly useful as a GitHub comment."],
            inputs_json,
        ),
        Some(DispatchOutcomeKind::NeedsUser) => no_reply(
            "needs_user_without_context_gap_reply",
            vec!["The dispatch outcome needs user input but did not produce a clarification draft."],
            inputs_json,
        ),
        None => no_reply(
            "missing_dispatch_outcome",
            vec!["No dispatch outcome was recorded for this run, so Issue Finder cannot justify a final GitHub reply."],
            inputs_json,
        ),
    }
}

fn has_context_gap(
    run: &DispatchRun,
    outcome: Option<&DispatchRunOutcome>,
    facts: &FinalCommentFacts,
) -> bool {
    run.status == DispatchRunStatus::NeedsUser
        || facts.outcome_kind == Some(DispatchOutcomeKind::NeedsUser)
        || facts.failure_class == Some(DispatchOutcomeFailureClass::ContextInsufficient)
        || outcome
            .and_then(|value| value.failure_class)
            .is_some_and(|class| class == DispatchOutcomeFailureClass::ContextInsufficient)
        || facts
            .result_status
            .as_deref()
            .is_some_and(is_context_gap_status)
}

fn is_context_gap_status(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().replace('-', "_").as_str(),
        "needs_user" | "needs_context" | "context_insufficient" | "clarification_needed"
    )
}

fn has_active_interaction(
    interactions: &[GitHubInteraction],
    interaction_type: GitHubInteractionType,
) -> bool {
    interactions.iter().any(|interaction| {
        interaction.interaction_type == interaction_type
            && interaction.status != GitHubInteractionStatus::Rejected
    })
}

fn interaction_inputs(interactions: &[GitHubInteraction]) -> Value {
    Value::Array(
        interactions
            .iter()
            .map(|interaction| {
                json!({
                    "id": interaction.id,
                    "type": interaction.interaction_type.as_str(),
                    "status": interaction.status.as_str(),
                    "githubCommentId": interaction.github_comment_id
                })
            })
            .collect(),
    )
}

fn draft(
    decision_kind: GitHubInteractionDecisionKind,
    interaction_type: GitHubInteractionType,
    body: String,
    reason_code: &str,
    reasons: Vec<&str>,
    inputs_json: Value,
) -> GitHubInteractionDecisionPlan {
    GitHubInteractionDecisionPlan {
        decision_kind,
        interaction_type: Some(interaction_type),
        body: Some(body),
        reason_code: reason_code.to_string(),
        reasons: reasons.into_iter().map(ToOwned::to_owned).collect(),
        inputs_json,
    }
}

fn no_comment(
    reason_code: &str,
    reasons: Vec<&str>,
    inputs_json: Value,
) -> GitHubInteractionDecisionPlan {
    GitHubInteractionDecisionPlan {
        decision_kind: GitHubInteractionDecisionKind::NoComment,
        interaction_type: None,
        body: None,
        reason_code: reason_code.to_string(),
        reasons: reasons.into_iter().map(ToOwned::to_owned).collect(),
        inputs_json,
    }
}

fn no_reply(
    reason_code: &str,
    reasons: Vec<&str>,
    inputs_json: Value,
) -> GitHubInteractionDecisionPlan {
    GitHubInteractionDecisionPlan {
        decision_kind: GitHubInteractionDecisionKind::NoReply,
        interaction_type: None,
        body: None,
        reason_code: reason_code.to_string(),
        reasons: reasons.into_iter().map(ToOwned::to_owned).collect(),
        inputs_json,
    }
}

fn normalized_body(value: Option<String>) -> Option<String> {
    value
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty())
}
