use anyhow::Result;
use serde::Serialize;

use super::model::{
    AgentCapabilityName, ApprovalType, CapabilityStatus, PolicyAction, PolicyRequirement,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDecision {
    pub action: PolicyAction,
    pub requirement: PolicyRequirement,
    pub approval_type: Option<ApprovalType>,
    pub required_capabilities: Vec<AgentCapabilityName>,
    pub reason: String,
}

pub fn classify_action(action: PolicyAction) -> PolicyDecision {
    match action {
        PolicyAction::StartDispatch => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::Dispatch),
            vec![
                AgentCapabilityName::StartSession,
                AgentCapabilityName::SetGoal,
                AgentCapabilityName::SetMetadata,
            ],
            "dispatch to an execution agent requires approval",
        ),
        PolicyAction::ResumeDispatch => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::Dispatch),
            vec![
                AgentCapabilityName::ResumeSession,
                AgentCapabilityName::SetGoal,
                AgentCapabilityName::SetMetadata,
            ],
            "resuming a native execution session for a task requires approval",
        ),
        PolicyAction::ReadSessionTranscript => decision(
            action,
            PolicyRequirement::Allowed,
            None,
            vec![AgentCapabilityName::ReadTranscript],
            "reading native transcript state is allowed when the adapter supports it",
        ),
        PolicyAction::RenameSession => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::SessionMutation),
            vec![AgentCapabilityName::RenameSession],
            "renaming a native session requires approval",
        ),
        PolicyAction::ForkSession => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::SessionMutation),
            vec![AgentCapabilityName::ForkSession],
            "forking a native session requires approval",
        ),
        PolicyAction::ArchiveSession => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::SessionMutation),
            vec![AgentCapabilityName::ArchiveSession],
            "archiving a native session requires approval",
        ),
        PolicyAction::SendA2aTask => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::A2aSend),
            Vec::new(),
            "sending A2A artifacts outside Issue Finder requires approval",
        ),
        PolicyAction::PostGithubComment => decision(
            action,
            PolicyRequirement::RequiresApproval,
            Some(ApprovalType::GithubPost),
            Vec::new(),
            "posting to GitHub requires approval",
        ),
        PolicyAction::OpenPr => decision(
            action,
            PolicyRequirement::Forbidden,
            Some(ApprovalType::OpenPr),
            vec![AgentCapabilityName::OpenPr],
            "Issue Finder must not create pull requests",
        ),
    }
}

pub fn ensure_capability_preconditions(
    store: &DispatchStore,
    agent_id: &str,
    decision: &PolicyDecision,
) -> Result<()> {
    if decision.requirement == PolicyRequirement::Forbidden {
        anyhow::bail!("{}", decision.reason);
    }
    for capability in &decision.required_capabilities {
        let capability_state = store.get_agent_capability(agent_id, *capability)?;
        if capability_state.status == CapabilityStatus::Unsupported {
            anyhow::bail!(
                "agent {agent_id} does not support capability {}",
                capability_state.capability.as_str()
            );
        }
    }
    Ok(())
}

fn decision(
    action: PolicyAction,
    requirement: PolicyRequirement,
    approval_type: Option<ApprovalType>,
    required_capabilities: Vec<AgentCapabilityName>,
    reason: &str,
) -> PolicyDecision {
    PolicyDecision {
        action,
        requirement,
        approval_type,
        required_capabilities,
        reason: reason.to_string(),
    }
}
