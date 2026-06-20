use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use super::adapters::NativeExecutionAdapter;
use super::model::{
    AgentSessionLink, ApprovalRequest, ApprovalStatus, ApprovalType, NewApprovalRequest,
};
use super::session_ops::{archive_session, fork_session, rename_session, SessionMutationResult};
use super::store::DispatchStore;

const ACTION_RENAME: &str = "rename";
const ACTION_FORK: &str = "fork";
const ACTION_ARCHIVE: &str = "archive";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMutationProposal {
    pub status: String,
    pub session: AgentSessionLink,
    pub approval_request: ApprovalRequest,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMutationApprovalResolution {
    pub approval_request: ApprovalRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mutation: Option<SessionMutationResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingSessionMutation {
    Rename {
        session_link_id: String,
        display_name: String,
    },
    Fork {
        session_link_id: String,
    },
    Archive {
        session_link_id: String,
    },
}

impl PendingSessionMutation {
    pub fn session_link_id(&self) -> &str {
        match self {
            Self::Rename {
                session_link_id, ..
            }
            | Self::Fork { session_link_id }
            | Self::Archive { session_link_id } => session_link_id,
        }
    }
}

pub fn request_session_rename(
    store: &DispatchStore,
    session_link_id: &str,
    display_name: &str,
) -> Result<SessionMutationProposal> {
    if display_name.trim().is_empty() {
        anyhow::bail!("session display name cannot be empty");
    }
    let session = store.get_session_link(session_link_id)?;
    let approval_request = store.create_approval_request(NewApprovalRequest {
        run_id: None,
        approval_type: ApprovalType::SessionMutation,
        status: ApprovalStatus::Pending,
        prompt: format!(
            "Rename native session {} to \"{}\"?",
            session.native_session_id,
            display_name.trim()
        ),
        details_json: json!({
            "action": ACTION_RENAME,
            "sessionLinkId": session.id,
            "displayName": display_name.trim()
        }),
    })?;
    Ok(SessionMutationProposal {
        status: "pending_approval".to_string(),
        session,
        approval_request,
    })
}

pub fn request_session_fork(
    store: &DispatchStore,
    session_link_id: &str,
) -> Result<SessionMutationProposal> {
    let session = store.get_session_link(session_link_id)?;
    let approval_request = store.create_approval_request(NewApprovalRequest {
        run_id: None,
        approval_type: ApprovalType::SessionMutation,
        status: ApprovalStatus::Pending,
        prompt: format!("Fork native session {}?", session.native_session_id),
        details_json: json!({
            "action": ACTION_FORK,
            "sessionLinkId": session.id
        }),
    })?;
    Ok(SessionMutationProposal {
        status: "pending_approval".to_string(),
        session,
        approval_request,
    })
}

pub fn request_session_archive(
    store: &DispatchStore,
    session_link_id: &str,
) -> Result<SessionMutationProposal> {
    let session = store.get_session_link(session_link_id)?;
    let approval_request = store.create_approval_request(NewApprovalRequest {
        run_id: None,
        approval_type: ApprovalType::SessionMutation,
        status: ApprovalStatus::Pending,
        prompt: format!("Archive native session {}?", session.native_session_id),
        details_json: json!({
            "action": ACTION_ARCHIVE,
            "sessionLinkId": session.id
        }),
    })?;
    Ok(SessionMutationProposal {
        status: "pending_approval".to_string(),
        session,
        approval_request,
    })
}

pub fn pending_session_mutation(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<PendingSessionMutation> {
    let approval = store.get_approval_request(approval_request_id)?;
    if approval.approval_type != ApprovalType::SessionMutation {
        anyhow::bail!(
            "approval request {} is {}, not session_mutation",
            approval.id,
            approval.approval_type
        );
    }
    if approval.status != ApprovalStatus::Pending {
        anyhow::bail!(
            "approval request {} is {}, not pending",
            approval.id,
            approval.status
        );
    }
    parse_session_mutation_details(&approval.details_json)
}

pub fn approve_session_mutation_with_adapter<A>(
    store: &DispatchStore,
    adapter: &mut A,
    approval_request_id: &str,
) -> Result<SessionMutationApprovalResolution>
where
    A: NativeExecutionAdapter,
{
    let mutation = pending_session_mutation(store, approval_request_id)?;
    let mutation_result = match mutation {
        PendingSessionMutation::Rename {
            session_link_id,
            display_name,
        } => rename_session(store, adapter, &session_link_id, &display_name)?,
        PendingSessionMutation::Fork { session_link_id } => {
            fork_session(store, adapter, &session_link_id)?
        }
        PendingSessionMutation::Archive { session_link_id } => {
            archive_session(store, adapter, &session_link_id)?
        }
    };
    let approval_request =
        store.resolve_approval_request(approval_request_id, ApprovalStatus::Approved)?;
    Ok(SessionMutationApprovalResolution {
        approval_request,
        mutation: Some(mutation_result),
    })
}

pub fn reject_session_mutation(
    store: &DispatchStore,
    approval_request_id: &str,
) -> Result<SessionMutationApprovalResolution> {
    pending_session_mutation(store, approval_request_id)?;
    let approval_request =
        store.resolve_approval_request(approval_request_id, ApprovalStatus::Rejected)?;
    Ok(SessionMutationApprovalResolution {
        approval_request,
        mutation: None,
    })
}

fn parse_session_mutation_details(value: &Value) -> Result<PendingSessionMutation> {
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .context("session mutation approval missing action")?;
    let session_link_id = value
        .get("sessionLinkId")
        .and_then(Value::as_str)
        .context("session mutation approval missing sessionLinkId")?
        .to_string();
    match action {
        ACTION_RENAME => {
            let display_name = value
                .get("displayName")
                .and_then(Value::as_str)
                .context("session rename approval missing displayName")?
                .to_string();
            Ok(PendingSessionMutation::Rename {
                session_link_id,
                display_name,
            })
        }
        ACTION_FORK => Ok(PendingSessionMutation::Fork { session_link_id }),
        ACTION_ARCHIVE => Ok(PendingSessionMutation::Archive { session_link_id }),
        _ => anyhow::bail!("unsupported session mutation action {action}"),
    }
}
