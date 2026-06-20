use anyhow::Result;

use super::cli_args::{
    AgentsArgs, AgentsCommand, DispatchA2aCommand, DispatchArgs, DispatchCommand,
    DispatchGithubCommand, DispatchOutcomeCommand, DispatchPackageCommand, SessionsArgs,
    SessionsCommand,
};
use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::model::{
    ApprovalStatus, DispatchFailureClass, DispatchOutcomeKind, DispatchRunStatus,
    DispatchTaskClass, DispatchValidationOutcome,
};
use super::output::*;
use super::runtime::{DispatchOutcomeRecordRequest, DispatchProposalRequest, DispatchRuntime};
use super::session_ops::SessionsSyncRequest;

pub fn handle_agents_cli(paths: &IssueFinderPaths, args: AgentsArgs) -> Result<String> {
    let runtime = DispatchRuntime::open(paths.clone())?;
    match args.command {
        AgentsCommand::List(args) => {
            let agents = runtime.list_agents()?;
            render_cli_output(args.json, &agents, || render_agents(&agents))
        }
        AgentsCommand::Capabilities(args) => {
            let capabilities = runtime.agent_capabilities(&args.agent)?;
            render_cli_output(args.json, &capabilities, || {
                render_agent_capabilities(&capabilities)
            })
        }
    }
}

pub fn handle_sessions_cli(paths: &IssueFinderPaths, args: SessionsArgs) -> Result<String> {
    let runtime = DispatchRuntime::open(paths.clone())?;
    match args.command {
        SessionsCommand::List(args) => {
            let sessions = runtime.list_sessions(args.agent.as_deref())?;
            render_cli_output(args.json, &sessions, || render_sessions(&sessions))
        }
        SessionsCommand::Sync(args) => {
            let result = runtime.sync_sessions(SessionsSyncRequest {
                agent_id: args.agent,
                search: args.search,
                limit: Some(args.limit),
            })?;
            render_cli_output(args.json, &result, || render_sessions_sync(&result))
        }
        SessionsCommand::Search(args) => {
            let result = runtime.search_sessions(&args.issue, args.agent.as_deref())?;
            render_cli_output(args.json, &result, || render_session_search(&result))
        }
        SessionsCommand::Read(args) => {
            let result = runtime.read_session_transcript(&args.session_link_id)?;
            render_cli_output(args.json, &result, || render_session_transcript(&result))
        }
        SessionsCommand::Rename(args) => {
            let result = runtime.rename_session(&args.session_link_id, &args.name)?;
            render_cli_output(args.json, &result, || {
                render_session_mutation_proposal(&result)
            })
        }
        SessionsCommand::Fork(args) => {
            let result = runtime.fork_session(&args.session_link_id)?;
            render_cli_output(args.json, &result, || {
                render_session_mutation_proposal(&result)
            })
        }
        SessionsCommand::Archive(args) => {
            let result = runtime.archive_session(&args.session_link_id)?;
            render_cli_output(args.json, &result, || {
                render_session_mutation_proposal(&result)
            })
        }
        SessionsCommand::Approve(args) => {
            let result = runtime.approve_session_mutation(&args.approval_request_id)?;
            render_cli_output(args.json, &result, || {
                render_session_mutation_approval(&result)
            })
        }
        SessionsCommand::Reject(args) => {
            let result = runtime.reject_session_mutation(&args.approval_request_id)?;
            render_cli_output(args.json, &result, || {
                render_session_mutation_approval(&result)
            })
        }
    }
}

pub fn handle_dispatch_cli(paths: &IssueFinderPaths, args: DispatchArgs) -> Result<String> {
    let runtime = DispatchRuntime::open(paths.clone())?;
    match args.command {
        None => {
            let issue = args
                .issue
                .ok_or_else(|| anyhow::anyhow!("dispatch requires an issue or subcommand"))?;
            propose_dispatch_cli(
                &runtime,
                issue,
                args.agent,
                args.new_session,
                args.session,
                args.json,
            )
        }
        Some(DispatchCommand::Package(args)) => match args.command {
            DispatchPackageCommand::ImportHandoff(args) => {
                let result = runtime.import_handoff_from_inbox(&args.inbox_id)?;
                render_cli_output(args.json, &result, || render_package_import(&result))
            }
        },
        Some(DispatchCommand::Propose(args)) => propose_dispatch_cli(
            &runtime,
            args.issue,
            args.agent,
            args.new_session,
            args.session,
            args.json,
        ),
        Some(DispatchCommand::Approve(args)) => {
            let result =
                runtime.resolve_dispatch_approval(&args.run_id, ApprovalStatus::Approved)?;
            render_cli_output(args.json, &result, || render_dispatch_approval(&result))
        }
        Some(DispatchCommand::Reject(args)) => {
            let result =
                runtime.resolve_dispatch_approval(&args.run_id, ApprovalStatus::Rejected)?;
            render_cli_output(args.json, &result, || render_dispatch_approval(&result))
        }
        Some(DispatchCommand::Execute(args)) => {
            let result = runtime.execute_dispatch(&args.run_id)?;
            render_cli_output(args.json, &result, || render_dispatch_execution(&result))
        }
        Some(DispatchCommand::A2a(args)) => match args.command {
            DispatchA2aCommand::Export(args) => {
                let result = runtime.export_a2a_task(&args.issue)?;
                render_cli_output(args.json, &result, || render_a2a_export(&result))
            }
            DispatchA2aCommand::Approve(args) => {
                let result = runtime.approve_a2a_send(&args.approval_request_id)?;
                render_cli_output(args.json, &result, || {
                    render_a2a_approval("approved", &result)
                })
            }
            DispatchA2aCommand::Reject(args) => {
                let result = runtime.reject_a2a_send(&args.approval_request_id)?;
                render_cli_output(args.json, &result, || {
                    render_a2a_approval("rejected", &result)
                })
            }
            DispatchA2aCommand::ImportResult(args) => {
                let args = *args;
                let status = args
                    .status
                    .as_deref()
                    .map(parse_dispatch_run_status)
                    .transpose()?;
                let outcome = optional_outcome_record_request(OptionalOutcomeRecordInput {
                    outcome: args.outcome.as_deref(),
                    failure_class: args.failure_class.as_deref(),
                    failure_reason: args.failure_reason.clone(),
                    task_class: args.task_class.as_deref(),
                    validation_outcome: args.validation_outcome.as_deref(),
                    idempotency_key: args.idempotency_key.clone(),
                    run_id: args.run_id.clone(),
                    result_artifact_id: None,
                })?;
                let result = runtime.import_a2a_result(
                    &args.run_id,
                    &args.path,
                    &args.kind,
                    &args.content_type,
                    status,
                    outcome,
                )?;
                render_cli_output(args.json, &result, || render_a2a_result_import(&result))
            }
        },
        Some(DispatchCommand::Outcome(args)) => match args.command {
            DispatchOutcomeCommand::Record(args) => {
                let result = runtime.record_dispatch_outcome(DispatchOutcomeRecordRequest {
                    run_id: args.run_id,
                    idempotency_key: args.idempotency_key,
                    outcome_kind: parse_dispatch_outcome_kind(&args.outcome)?,
                    failure_class: args
                        .failure_class
                        .as_deref()
                        .map(parse_dispatch_failure_class)
                        .transpose()?,
                    failure_detail: args.failure_reason,
                    task_class: args
                        .task_class
                        .as_deref()
                        .map(parse_dispatch_task_class)
                        .transpose()?,
                    validation_outcome: args
                        .validation_outcome
                        .as_deref()
                        .map(parse_dispatch_validation_outcome)
                        .transpose()?,
                    result_artifact_id: args.result_artifact_id,
                    metadata_json: serde_json::json!({ "source": "cli_dispatch_outcome_record" }),
                })?;
                render_cli_output(args.json, &result, || {
                    render_dispatch_outcome_record(&result)
                })
            }
        },
        Some(DispatchCommand::Github(args)) => match args.command {
            DispatchGithubCommand::DraftTracking(args) => {
                let result = runtime.draft_github_tracking_comment(&args.issue, args.body)?;
                render_cli_output(args.json, &result, || render_github_draft(&result))
            }
            DispatchGithubCommand::DraftFinal(args) => {
                let result = runtime.draft_github_final_comment(&args.run_id, args.body)?;
                render_cli_output(args.json, &result, || render_github_draft(&result))
            }
            DispatchGithubCommand::Approve(args) => {
                let result = runtime.approve_github_interaction(&args.interaction_id)?;
                render_cli_output(args.json, &result, || {
                    render_github_approval("approved", &result)
                })
            }
            DispatchGithubCommand::Reject(args) => {
                let result = runtime.reject_github_interaction(&args.interaction_id)?;
                render_cli_output(args.json, &result, || {
                    render_github_approval("rejected", &result)
                })
            }
            DispatchGithubCommand::Post(args) => {
                let config = Config::load_or_default(paths)?;
                let result = runtime.post_github_interaction(&config, &args.interaction_id)?;
                render_cli_output(args.json, &result, || render_github_post(&result))
            }
            DispatchGithubCommand::Retry(args) => {
                let config = Config::load_or_default(paths)?;
                let result = runtime.retry_github_interaction(&config, &args.interaction_id)?;
                render_cli_output(args.json, &result, || render_github_post(&result))
            }
            DispatchGithubCommand::List(args) => {
                let interactions = runtime.list_github_interactions(&args.issue)?;
                render_cli_output(args.json, &interactions, || {
                    render_github_interactions(&interactions)
                })
            }
        },
        Some(DispatchCommand::Status(args)) => {
            let status = runtime.dispatch_status(&args.run_id)?;
            render_cli_output(args.json, &status, || render_dispatch_status(&status))
        }
        Some(DispatchCommand::Events(args)) => {
            let events = runtime.dispatch_events(&args.run_id)?;
            render_cli_output(args.json, &events, || render_dispatch_events(&events))
        }
        Some(DispatchCommand::Artifacts(args)) => {
            let artifacts = runtime.dispatch_artifacts(&args.run_id)?;
            render_cli_output(args.json, &artifacts, || {
                render_dispatch_artifacts(&artifacts)
            })
        }
    }
}

fn propose_dispatch_cli(
    runtime: &DispatchRuntime,
    issue: String,
    agent: String,
    new_session: bool,
    session: Option<String>,
    json: bool,
) -> Result<String> {
    let proposal = runtime.propose_dispatch(DispatchProposalRequest {
        issue,
        agent_id: agent,
        requested_by: "cli".to_string(),
        selected_session_link_id: session,
        new_session,
    })?;
    render_cli_output(json, &proposal, || render_dispatch_proposal(&proposal))
}

fn parse_dispatch_run_status(value: &str) -> Result<DispatchRunStatus> {
    DispatchRunStatus::parse_value(value)
        .ok_or_else(|| anyhow::anyhow!("invalid dispatch status {value}"))
}

struct OptionalOutcomeRecordInput<'a> {
    outcome: Option<&'a str>,
    failure_class: Option<&'a str>,
    failure_reason: Option<String>,
    task_class: Option<&'a str>,
    validation_outcome: Option<&'a str>,
    idempotency_key: Option<String>,
    run_id: String,
    result_artifact_id: Option<String>,
}

fn optional_outcome_record_request(
    input: OptionalOutcomeRecordInput<'_>,
) -> Result<Option<DispatchOutcomeRecordRequest>> {
    let Some(outcome) = input.outcome else {
        return Ok(None);
    };
    Ok(Some(DispatchOutcomeRecordRequest {
        run_id: input.run_id,
        idempotency_key: input.idempotency_key,
        outcome_kind: parse_dispatch_outcome_kind(outcome)?,
        failure_class: input
            .failure_class
            .map(parse_dispatch_failure_class)
            .transpose()?,
        failure_detail: input.failure_reason,
        task_class: input
            .task_class
            .map(parse_dispatch_task_class)
            .transpose()?,
        validation_outcome: input
            .validation_outcome
            .map(parse_dispatch_validation_outcome)
            .transpose()?,
        result_artifact_id: input.result_artifact_id,
        metadata_json: serde_json::json!({ "source": "cli_a2a_import_result" }),
    }))
}

fn parse_dispatch_outcome_kind(value: &str) -> Result<DispatchOutcomeKind> {
    DispatchOutcomeKind::parse_value(value)
        .ok_or_else(|| anyhow::anyhow!("invalid dispatch outcome kind {value}"))
}

fn parse_dispatch_failure_class(value: &str) -> Result<DispatchFailureClass> {
    DispatchFailureClass::parse_value(value)
        .ok_or_else(|| anyhow::anyhow!("invalid dispatch failure class {value}"))
}

fn parse_dispatch_task_class(value: &str) -> Result<DispatchTaskClass> {
    DispatchTaskClass::parse_value(value)
        .ok_or_else(|| anyhow::anyhow!("invalid dispatch task class {value}"))
}

fn parse_dispatch_validation_outcome(value: &str) -> Result<DispatchValidationOutcome> {
    DispatchValidationOutcome::parse_value(value)
        .ok_or_else(|| anyhow::anyhow!("invalid dispatch validation outcome {value}"))
}
