use anyhow::Result;

use super::a2a_gateway::{A2aApprovalResult, A2aExportResult, A2aResultImport};
use super::execution::DispatchExecutionResult;
use super::github_projection::{GitHubApprovalResult, GitHubCommentDraftResult, GitHubPostResult};
use super::model::{AgentArtifact, AgentEvent, AgentProfile, AgentSessionLink, GitHubInteraction};
use super::packaging::PackageImportResult;
use super::runtime::{
    AgentCapabilitiesView, DispatchApprovalResolution, DispatchProposal, DispatchStatusSnapshot,
    SessionSearchResult,
};
use super::session_approvals::{SessionMutationApprovalResolution, SessionMutationProposal};
use super::session_ops::{SessionTranscriptResult, SessionsSyncResult};

pub(crate) fn render_cli_output<T: serde::Serialize>(
    json: bool,
    value: &T,
    text: impl FnOnce() -> String,
) -> Result<String> {
    if json {
        Ok(serde_json::to_string_pretty(value)?)
    } else {
        Ok(text())
    }
}

pub(crate) fn render_agents(agents: &[AgentProfile]) -> String {
    if agents.is_empty() {
        return "No agents configured.".to_string();
    }

    let mut lines = vec!["Agents:".to_string()];
    for agent in agents {
        lines.push(format!(
            "- {} ({}) adapter={} enabled={}",
            agent.id, agent.display_name, agent.adapter, agent.enabled
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_agent_capabilities(view: &AgentCapabilitiesView) -> String {
    let mut lines = vec![format!(
        "Capabilities for {} ({})",
        view.agent.id, view.agent.display_name
    )];
    for capability in &view.capabilities {
        lines.push(format!(
            "- {}: {}",
            capability.capability, capability.status
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_sessions(sessions: &[AgentSessionLink]) -> String {
    if sessions.is_empty() {
        return "No local session links found.".to_string();
    }

    let mut lines = vec!["Sessions:".to_string()];
    for session in sessions {
        lines.push(format!(
            "- {} agent={} native={} status={} issue={}",
            session.id,
            session.agent_id,
            session.native_session_id,
            session.status,
            session.issue_task_id.as_deref().unwrap_or("-")
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_sessions_sync(result: &SessionsSyncResult) -> String {
    if result.synced.is_empty() {
        return format!("No native sessions synced for {}.", result.agent_id);
    }

    let mut lines = vec![format!(
        "Synced {} native sessions for {}:",
        result.synced.len(),
        result.agent_id
    )];
    for session in &result.synced {
        lines.push(format!(
            "- {} native={} name={}",
            session.id, session.native_session_id, session.display_name
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_session_search(result: &SessionSearchResult) -> String {
    if result.sessions.is_empty() {
        if result.issue_task_found {
            return format!("No local session links found for {}.", result.issue_key);
        }
        return format!(
            "No dispatch issue task has been imported for {}; no local session links found.",
            result.issue_key
        );
    }

    let mut lines = vec![format!("Sessions for {}:", result.issue_key)];
    for session in &result.sessions {
        lines.push(format!(
            "- {} agent={} native={} status={}",
            session.id, session.agent_id, session.native_session_id, session.status
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_session_transcript(result: &SessionTranscriptResult) -> String {
    format!(
        "Read session {} transcript into artifact {}.\nPath: {}",
        result.session.id, result.transcript_artifact.id, result.transcript_artifact.path
    )
}

pub(crate) fn render_session_mutation_proposal(result: &SessionMutationProposal) -> String {
    format!(
        "Session mutation for {} is pending approval.\nApproval request: {}",
        result.session.id, result.approval_request.id
    )
}

pub(crate) fn render_session_mutation_approval(
    result: &SessionMutationApprovalResolution,
) -> String {
    match &result.mutation {
        Some(mutation) => format!(
            "Session mutation {} approved and executed.\nSession {} is {}.",
            result.approval_request.id, mutation.session.id, mutation.session.status
        ),
        None => format!(
            "Session mutation {} is {}.",
            result.approval_request.id, result.approval_request.status
        ),
    }
}

pub(crate) fn render_dispatch_status(status: &DispatchStatusSnapshot) -> String {
    let mut lines = vec![
        format!("Dispatch run {}: {}", status.run.id, status.run.status),
        format!(
            "Issue: {}#{} {}",
            status.issue_task.repo_full_name,
            status.issue_task.issue_number,
            status.issue_task.title
        ),
        format!("Agent: {} ({})", status.agent.id, status.agent.display_name),
        format!("Approval: {}", status.run.approval_state),
    ];
    if let Some(session) = &status.selected_session {
        lines.push(format!(
            "Session: {} native={} status={}",
            session.id, session.native_session_id, session.status
        ));
    }
    lines.push(format!("Approvals: {}", status.approval_requests.len()));
    lines.push(format!("Artifacts: {}", status.artifacts.len()));
    if let Some(reason) = &status.run.failure_reason {
        lines.push(format!("Failure: {reason}"));
    }
    lines.join("\n")
}

pub(crate) fn render_dispatch_events(events: &[AgentEvent]) -> String {
    if events.is_empty() {
        return "No dispatch events found.".to_string();
    }

    let mut lines = vec!["Dispatch events:".to_string()];
    for event in events {
        lines.push(format!(
            "- {} {} native={}",
            event.created_at,
            event.event_type,
            event.native_event_id.as_deref().unwrap_or("-")
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_dispatch_artifacts(artifacts: &[AgentArtifact]) -> String {
    if artifacts.is_empty() {
        return "No dispatch artifacts found.".to_string();
    }

    let mut lines = vec!["Dispatch artifacts:".to_string()];
    for artifact in artifacts {
        lines.push(format!(
            "- {} kind={} contentType={} path={}",
            artifact.id, artifact.kind, artifact.content_type, artifact.path
        ));
    }
    lines.join("\n")
}

pub(crate) fn render_package_import(result: &PackageImportResult) -> String {
    format!(
        "Imported {}#{} into task package {}.\nHandoff artifact: {}\nPackage artifact: {}",
        result.issue_task.repo_full_name,
        result.issue_task.issue_number,
        result.package.issue.title,
        result.handoff_artifact.path,
        result.package_artifact.path
    )
}

pub(crate) fn render_dispatch_proposal(proposal: &DispatchProposal) -> String {
    format!(
        "Dispatch proposal {} is pending approval.\nIssue: {}#{}\nApproval request: {}",
        proposal.run.id,
        proposal.issue_task.repo_full_name,
        proposal.issue_task.issue_number,
        proposal.approval_request.id
    )
}

pub(crate) fn render_dispatch_approval(result: &DispatchApprovalResolution) -> String {
    format!(
        "Dispatch run {} approval is {}.\nRun status: {}",
        result.run.id, result.run.approval_state, result.run.status
    )
}

pub(crate) fn render_dispatch_execution(result: &DispatchExecutionResult) -> String {
    format!(
        "Dispatch run {} started native turn {}.\nSession: {} native={}\nPrompt artifact: {}",
        result.run.id,
        result.turn.native_turn_id,
        result.session.id,
        result.session.native_session_id,
        result.prompt_artifact.path
    )
}

pub(crate) fn render_a2a_export(result: &A2aExportResult) -> String {
    format!(
        "Created local A2A task artifact for {} and queued outbound approval.\nTask: {}\nPath: {}\nApproval request: {}",
        result.task.task.issue_key,
        result.task.task.task_type,
        result.export_artifact.path,
        result.approval_request.id
    )
}

pub(crate) fn render_a2a_approval(action: &str, result: &A2aApprovalResult) -> String {
    format!(
        "A2A task artifact {} is {} for {}.\nApproval request: {}",
        result.export_artifact.id, action, result.task.task.issue_key, result.approval_request.id
    )
}

pub(crate) fn render_a2a_result_import(result: &A2aResultImport) -> String {
    format!(
        "Imported A2A result artifact {} for dispatch run {}.\nRun status: {}",
        result.artifact.id, result.run.id, result.run.status
    )
}

pub(crate) fn render_github_draft(result: &GitHubCommentDraftResult) -> String {
    format!(
        "Drafted {} for {}#{}.\nInteraction: {}\nApproval request: {}\nBody artifact: {}",
        result.interaction.interaction_type,
        result.issue_task.repo_full_name,
        result.issue_task.issue_number,
        result.interaction.id,
        result.approval_request.id,
        result.body_artifact.path
    )
}

pub(crate) fn render_github_approval(action: &str, result: &GitHubApprovalResult) -> String {
    format!(
        "GitHub interaction {} is {}.\nApproval request: {}",
        result.interaction.id, action, result.approval_request.id
    )
}

pub(crate) fn render_github_post(result: &GitHubPostResult) -> String {
    format!(
        "Posted GitHub interaction {} as comment {}.\nURL: {}",
        result.interaction.id, result.posted_comment.id, result.posted_comment.url
    )
}

pub(crate) fn render_github_interactions(interactions: &[GitHubInteraction]) -> String {
    if interactions.is_empty() {
        return "No GitHub interactions found.".to_string();
    }

    let mut lines = vec!["GitHub interactions:".to_string()];
    for interaction in interactions {
        lines.push(format!(
            "- {} type={} status={} comment={}",
            interaction.id,
            interaction.interaction_type,
            interaction.status,
            interaction.github_comment_id.as_deref().unwrap_or("-")
        ));
    }
    lines.join("\n")
}
