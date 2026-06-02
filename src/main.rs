use anyhow::Result;
use clap::Parser;
use patchbay_cli::cli::{Cli, Command, InboxCommand};
use patchbay_cli::config::{initialize_interactive, Config};
use patchbay_cli::doctor;
use patchbay_cli::inbox::{self, InboxStatus};
use patchbay_cli::paths::PatchbayPaths;
use patchbay_cli::workflow;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let paths = PatchbayPaths::resolve()?;

    match cli.command {
        Command::Init(args) => {
            let config = initialize_interactive(&paths, args.force)?;
            println!("Patchbay initialized at {}", paths.home.display());
            if config.github.token.trim().is_empty() {
                println!(
                    "GitHub token is empty; `scout`, `prepare`, and `daily` may hit API limits."
                );
            }
        }
        Command::Scout(args) => {
            let config = Config::load(&paths)?;
            let ranked = workflow::scout(&paths, &config, args.limit, args.refresh).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&ranked)?);
            } else {
                println!("{}", workflow::render_ranked(&ranked));
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
                println!("{}", inbox::render_index(&index));
            }
            Some(InboxCommand::Done { inbox_id }) => {
                let index = inbox::update_status(&paths, &inbox_id, InboxStatus::Done)?;
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
        Command::Daily(args) => {
            let config = Config::load(&paths)?;
            let (report, path) = workflow::daily(&paths, &config, args.top, args.refresh).await?;
            println!("{}", workflow::render_daily(&report, &path));
        }
        Command::Report(args) => {
            println!("{}", workflow::read_report(&paths, args.date)?);
        }
        Command::Doctor => {
            doctor::ensure_paths(&paths)?;
            let config = Config::load_or_default(&paths)?;
            let checks = doctor::run_doctor(&paths, Some(&config)).await;
            println!("{}", doctor::render_doctor(&checks));
        }
    }

    Ok(())
}
