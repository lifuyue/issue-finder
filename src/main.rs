use anyhow::Result;
use clap::Parser;
use issue_finder::cli::{
    Cli, Command, FeedbackCommand, InboxCommand, ProfileCommand, ToolsCommand,
};
use issue_finder::config::{initialize_interactive, Config};
use issue_finder::doctor;
use issue_finder::inbox::{self, InboxStatus};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::profile_bootstrap::{bootstrap_profile, render_profile_bootstrap_report};
use issue_finder::recommendation::{
    record_event_for_key, DiscoveryScope, IssueKey, RecommendationEventSource,
    RecommendationEventType, RepositoryScope, ScoutOptions,
};
use issue_finder::tool_runtime::{
    default_call_id, IssueFinderToolInvocation, IssueFinderToolOutput, IssueFinderToolRuntime,
};
use issue_finder::tool_specs::list_tool_specs;
use issue_finder::workflow;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let paths = IssueFinderPaths::resolve()?;

    match cli.command {
        Command::Init(args) => {
            let config = initialize_interactive(&paths, args.force)?;
            println!("Issue Finder initialized at {}", paths.home.display());
            if config.github.token.trim().is_empty() {
                println!(
                    "GitHub token is empty; `scout`, `prepare`, and `daily` may hit API limits."
                );
            }
        }
        Command::Scout(args) => {
            let config = Config::load(&paths)?;
            let scope = discovery_scope(args.repo)?;
            let result = workflow::scout_with_options(
                &paths,
                &config,
                args.limit,
                args.refresh,
                ScoutOptions {
                    include_filtered: false,
                    record_exposure: !args.dry_run,
                    source: RecommendationEventSource::CliScout,
                },
                scope,
            )
            .await?;
            if args.stats_json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else if args.json {
                println!("{}", serde_json::to_string_pretty(&result.ranked)?);
            } else {
                println!("{}", workflow::render_ranked(&result.ranked));
            }
        }
        Command::Assess(args) => {
            let config = Config::load(&paths)?;
            let ranked = workflow::assess_issue_selection_with_options(
                &paths,
                &config,
                workflow::IssueSelector::new(args.issue, args.url),
                args.refresh,
                !args.dry_run,
                RecommendationEventSource::CliAssess,
            )
            .await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&ranked)?);
            } else {
                println!("{}", workflow::render_ranked(&[ranked]));
            }
        }
        Command::Prepare(args) => {
            let config = Config::load(&paths)?;
            let outcome =
                workflow::prepare_from_input(&paths, &config, args.issue, args.url).await?;
            println!("{}", workflow::render_prepare_outcome(&outcome));
        }
        Command::Handoff(args) => {
            let use_json = args.json && !args.print;
            let contents = workflow::read_handoff(&paths, &args.inbox_id, use_json)?;
            println!("{contents}");
        }
        Command::Inbox(args) => match args.command {
            Some(InboxCommand::Archive { inbox_id }) => {
                let index = inbox::update_status(&paths, &inbox_id, InboxStatus::Archived)?;
                if let Some(item) = index.items.iter().find(|item| item.id == inbox_id) {
                    let _ = record_event_for_key(
                        &paths,
                        IssueKey::new(item.repo_full_name.clone(), item.issue_number),
                        RecommendationEventType::Dismissed,
                        RecommendationEventSource::InboxArchive,
                    );
                }
                println!("{}", inbox::render_index(&index));
            }
            Some(InboxCommand::Done { inbox_id }) => {
                let index = inbox::update_status(&paths, &inbox_id, InboxStatus::Done)?;
                if let Some(item) = index.items.iter().find(|item| item.id == inbox_id) {
                    let _ = record_event_for_key(
                        &paths,
                        IssueKey::new(item.repo_full_name.clone(), item.issue_number),
                        RecommendationEventType::Done,
                        RecommendationEventSource::InboxDone,
                    );
                }
                println!("{}", inbox::render_index(&index));
            }
            None => {
                let index = inbox::load_index(&paths)?;
                if args.json {
                    println!("{}", serde_json::to_string_pretty(&index)?);
                } else {
                    println!("{}", inbox::render_index(&index));
                }
            }
        },
        Command::Feedback(args) => match args.command {
            FeedbackCommand::Read { issue } => {
                println!(
                    "{}",
                    workflow::record_feedback(&paths, &issue, RecommendationEventType::Read)?
                );
            }
            FeedbackCommand::Dismiss { issue } => {
                println!(
                    "{}",
                    workflow::record_feedback(&paths, &issue, RecommendationEventType::Dismissed)?
                );
            }
            FeedbackCommand::Restore { issue } => {
                println!(
                    "{}",
                    workflow::record_feedback(&paths, &issue, RecommendationEventType::Restored)?
                );
            }
            FeedbackCommand::Show { issue } => {
                println!("{}", workflow::render_feedback_state(&paths, &issue)?);
            }
        },
        Command::Daily(args) => {
            let config = Config::load(&paths)?;
            let scope = discovery_scope(args.repo)?;
            let (report, path) =
                workflow::daily(&paths, &config, args.top, args.refresh, scope).await?;
            println!("{}", workflow::render_daily(&report, &path));
        }
        Command::Report(args) => {
            println!("{}", workflow::read_report(&paths, args.date)?);
        }
        Command::Profile(args) => match args.command {
            ProfileCommand::Bootstrap(args) => {
                let scan_root = match args.scan_root {
                    Some(path) => path,
                    None => dirs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("unable to determine home directory"))?,
                };
                let report = bootstrap_profile(&scan_root)?;
                if args.json {
                    println!("{}", serde_json::to_string(&report)?);
                } else {
                    println!("{}", render_profile_bootstrap_report(&report));
                }
            }
        },
        Command::Eval(args) => match args.command {
            issue_finder::cli::EvalCommand::Recommendation(eval_args) => {
                if !eval_args.offline && !eval_args.live {
                    anyhow::bail!("choose either --offline or --live");
                }
                if eval_args.offline {
                    let report =
                        issue_finder::recommendation::eval::run_offline_eval(&eval_args.output)?;
                    println!(
                        "Wrote offline recommendation eval to {} ({} samples).",
                        eval_args.output.display(),
                        report.overall.samples
                    );
                } else {
                    let config = Config::load(&paths)?;
                    let report = issue_finder::recommendation::eval::run_live_eval(
                        &config,
                        eval_args.limit,
                        eval_args.refresh,
                        &eval_args.output,
                    )
                    .await?;
                    println!(
                        "Wrote live recommendation eval to {} ({} visible candidates).",
                        eval_args.output.display(),
                        report.summary.total_visible
                    );
                }
            }
        },
        Command::Tools(args) => match args.command {
            ToolsCommand::List => {
                println!("{}", serde_json::to_string(&list_tool_specs())?);
            }
            ToolsCommand::Call(args) => {
                let call_id = args.call_id.unwrap_or_else(default_call_id);
                let invocation = IssueFinderToolInvocation::from_json_arguments(
                    args.tool.clone(),
                    &args.arguments,
                    Some(call_id.clone()),
                    args.turn_id.clone(),
                );
                let output = match invocation {
                    Ok(invocation) => match Config::load_or_default(&paths) {
                        Ok(config) => {
                            IssueFinderToolRuntime::new(paths.clone(), config)
                                .execute(invocation)
                                .await
                        }
                        Err(error) if args.tool == "issue-finder.status" => {
                            IssueFinderToolRuntime::new_with_config_load_error(
                                paths.clone(),
                                Config::default(),
                                Some(error.to_string()),
                            )
                            .execute(invocation)
                            .await
                        }
                        Err(error) => IssueFinderToolOutput::failure(
                            call_id,
                            args.turn_id,
                            args.tool,
                            "system_error",
                            error.to_string(),
                        ),
                    },
                    Err(error) => IssueFinderToolOutput::failure(
                        call_id,
                        args.turn_id,
                        args.tool,
                        "invalid_arguments",
                        error,
                    ),
                };
                println!("{}", serde_json::to_string(&output)?);
            }
        },
        Command::Doctor => {
            doctor::ensure_paths(&paths)?;
            let config = Config::load_or_default(&paths)?;
            let checks = doctor::run_doctor(&paths, Some(&config)).await;
            println!("{}", doctor::render_doctor(&checks));
        }
    }

    Ok(())
}

fn discovery_scope(repo: Option<String>) -> Result<DiscoveryScope> {
    match repo {
        Some(repo) => Ok(DiscoveryScope::repository(RepositoryScope::parse(&repo)?)),
        None => Ok(DiscoveryScope::Global),
    }
}
