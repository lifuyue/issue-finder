use anyhow::Result;

use super::cli_args::{
    AgentsArgs, AgentsCommand, DispatchA2aCommand, DispatchArgs, DispatchCommand,
    DispatchGithubCommand, DispatchPackageCommand, DispatchReviewCommand, SessionsArgs,
    SessionsCommand,
};
use crate::config::Config;
use crate::paths::IssueFinderPaths;

use super::model::{ApprovalStatus, DispatchRunStatus};
use super::output::*;
use super::runtime::{DispatchProposalRequest, DispatchRuntime};
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
        AgentsCommand::Probe(args) => {
            let result = runtime.probe_agent(&args.agent, args.refresh)?;
            render_cli_output(args.json, &result, || render_agent_probe(&result))
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
        SessionsCommand::Replay(args) => {
            let result = runtime.session_replay(&args.session_link_id)?;
            render_cli_output(args.json, &result, || render_session_replay(&result))
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
        Some(DispatchCommand::Review(args)) => match args.command {
            DispatchReviewCommand::List(args) => {
                let result = runtime.list_issue_reviews()?;
                render_cli_output(args.json, &result, || render_issue_reviews(&result))
            }
            DispatchReviewCommand::Show(args) => {
                let result = runtime.show_issue_review(&args.approval_request_id)?;
                render_cli_output(args.json, &result, || render_issue_review(&result))
            }
            DispatchReviewCommand::Approve(args) => {
                let result = runtime.approve_issue_review(&args.approval_request_id)?;
                render_cli_output(args.json, &result, || {
                    render_issue_review_resolution("approved", &result)
                })
            }
            DispatchReviewCommand::Reject(args) => {
                let result = runtime.reject_issue_review(&args.approval_request_id, args.reason)?;
                render_cli_output(args.json, &result, || {
                    render_issue_review_resolution("rejected", &result)
                })
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
                let status = args
                    .status
                    .as_deref()
                    .map(parse_dispatch_run_status)
                    .transpose()?;
                let result = runtime.import_a2a_result(
                    &args.run_id,
                    &args.path,
                    &args.kind,
                    &args.content_type,
                    status,
                )?;
                render_cli_output(args.json, &result, || render_a2a_result_import(&result))
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
        Some(DispatchCommand::Timeline(args)) => {
            let timeline = runtime.dispatch_timeline(&args.run_id)?;
            render_cli_output(args.json, &timeline, || render_dispatch_timeline(&timeline))
        }
        Some(DispatchCommand::Trace(args)) => {
            let trace = runtime.dispatch_trace(&args.run_id)?;
            render_cli_output(args.json, &trace, || render_dispatch_trace(&trace))
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
