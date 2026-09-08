use super::*;
use crate::discovery::SearchOptions;
use crate::github::{IssueContext, IssueRef};
use crate::recommendation::{RecommendationEngine, RecommendationEventType};
use crate::tool_specs::{TOOL_FEEDBACK, TOOL_FINISH, TOOL_TASK_STATUS};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProfileArgs {
    tech_stack: Option<Vec<String>>,
    keywords: Option<Vec<String>>,
}

impl ProfileArgs {
    fn apply(self, config: &mut Config) -> RuntimeResult<()> {
        for (value, target) in [
            (self.tech_stack, &mut config.profile.tech_stack),
            (self.keywords, &mut config.profile.keywords),
        ] {
            if let Some(terms) = value {
                if terms.len() > 20
                    || terms
                        .iter()
                        .any(|term| term.len() > 100 || term.trim().is_empty())
                {
                    return Err(invalid(
                        "profile accepts at most 20 nonempty terms of up to 100 bytes",
                    ));
                }
                *target = terms
                    .into_iter()
                    .map(|term| term.trim().to_string())
                    .collect();
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScoutArgs {
    repo: Option<String>,
    limit: Option<usize>,
    search: Option<SearchOptions>,
    #[serde(default)]
    profile: ProfileArgs,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    include_filtered: bool,
    record_exposure: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssessArgs {
    issue: Option<String>,
    url: Option<String>,
    #[serde(default)]
    refresh: bool,
    record_read: Option<bool>,
    #[serde(default)]
    profile: ProfileArgs,
    comments_page: Option<usize>,
    comments_per_page: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrepareArgs {
    issue: Option<String>,
    url: Option<String>,
    checkout: Option<PathBuf>,
    workspace_root: Option<PathBuf>,
    #[serde(default)]
    profile: ProfileArgs,
    #[serde(default)]
    allow_gate_bypass: bool,
    bypass_reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceArgs {
    workspace: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FinishArgs {
    workspace: PathBuf,
    checks: Vec<Vec<String>>,
    check_timeout_seconds: Option<u64>,
    summary: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FeedbackArgs {
    issue: String,
    action: String,
}

impl IssueFinderToolRuntime {
    pub(super) async fn call_session_tool(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        if self.config_load_error.is_some() && invocation.tool_name != TOOL_STATUS {
            return Err(invalid(
                "configuration is invalid; call issue-finder.status for details",
            ));
        }
        match invocation.tool_name.as_str() {
            TOOL_STATUS => self.session_status(invocation).await,
            TOOL_SCOUT => self.session_scout(invocation).await,
            TOOL_ASSESS => self.session_assess(invocation).await,
            TOOL_PREPARE => self.session_prepare(invocation).await,
            TOOL_TASK_STATUS => {
                let args: WorkspaceArgs = parse_arguments(&invocation.arguments)?;
                absolute_path(&args.workspace)?;
                let paths = self.paths.clone();
                let task = tokio::task::spawn_blocking(move || {
                    crate::session::task_status(&paths, &args.workspace)
                })
                .await
                .map_err(|error| RuntimeFailure::System(error.into()))??;
                Ok(output(invocation, &task.status, to_value(&task), true))
            }
            TOOL_FINISH => {
                let args: FinishArgs = parse_arguments(&invocation.arguments)?;
                absolute_path(&args.workspace)?;
                let timeout = args.check_timeout_seconds.unwrap_or(600);
                if args.checks.is_empty()
                    || args.checks.len() > 20
                    || !(1..=3600).contains(&timeout)
                    || args
                        .checks
                        .iter()
                        .any(|argv| argv.is_empty() || argv[0].trim().is_empty())
                {
                    return Err(invalid(
                        "finish needs 1..20 nonempty argv checks and a timeout of 1..3600 seconds",
                    ));
                }
                let paths = self.paths.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::session::finish(
                        &paths,
                        &args.workspace,
                        &crate::session::FinishOptions {
                            checks: args.checks,
                            check_timeout_seconds: timeout,
                            summary: args.summary,
                        },
                    )
                })
                .await
                .map_err(|error| RuntimeFailure::System(error.into()))??;
                Ok(output(
                    invocation,
                    &result.status,
                    to_value(&result),
                    result.status == "completed",
                ))
            }
            TOOL_FEEDBACK => {
                let args: FeedbackArgs = parse_arguments(&invocation.arguments)?;
                valid_issue(Some(args.issue.clone()), None)?;
                let action = match args.action.as_str() {
                    "read" => RecommendationEventType::Read,
                    "dismiss" => RecommendationEventType::Dismissed,
                    "restore" => RecommendationEventType::Restored,
                    _ => return Err(invalid("feedback action must be read, dismiss or restore")),
                };
                let message = workflow::record_feedback(&self.paths, &args.issue, action)?;
                Ok(output(
                    invocation,
                    "recorded",
                    json!({"issue":args.issue,"action":args.action,"message":message}),
                    true,
                ))
            }
            _ => Err(invalid("tool is not part of the current-session workflow")),
        }
    }

    fn session_config(&self, profile: ProfileArgs) -> RuntimeResult<Config> {
        let mut config = self.config.clone();
        config.github.token = config.resolved_session_github_token().token;
        // Semantic review belongs to the current agent; never invoke a second LLM from prepare.
        config.llm.enabled = false;
        profile.apply(&mut config)?;
        Ok(config)
    }

    async fn session_status(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: StatusToolArgs = parse_arguments(&invocation.arguments)?;
        let token = self.config.resolved_session_github_token();
        let mut config = self.config.clone();
        config.github.token = token.token;
        let mut error = self.config_load_error.clone();
        let mut login = None;
        let check_auth = args.check_auth.unwrap_or(true);
        if error.is_none() && token.source == GitHubTokenSource::Missing {
            error = Some("GitHub authentication is unavailable; run gh auth login in this execution environment.".into());
        }
        if error.is_none() && check_auth {
            match GitHubClient::new(&config)?.validate_token().await {
                Ok(value) => login = Some(value),
                Err(value) => error = Some(value.to_string()),
            }
        }
        let status = if self.config_load_error.is_some() {
            "invalid_config"
        } else if error.is_some() {
            "auth_required"
        } else {
            "ready"
        };
        Ok(output(
            invocation,
            status,
            json!({
                "cliVersion":env!("CARGO_PKG_VERSION"), "sessionContractVersion":1,
                "config":{"path":self.paths.config,"exists":self.paths.config.exists(),"loadError":self.config_load_error,"optional":true},
                "github":{"tokenSource":token.source.as_str(),"authChecked":check_auth,"login":login,"error":error},
                "stateRoot":self.paths.home,"defaultWorkspaceRoot":default_workspace_root(&self.paths),
                "nextAction": if status == "ready" { "Call scout or assess; no init is needed." } else if status == "invalid_config" { "Repair the reported config file without overwriting unrelated settings." } else { "Authenticate GitHub in this environment, then retry." }
            }),
            status == "ready",
        ))
    }

    async fn session_scout(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: ScoutArgs = parse_arguments(&invocation.arguments)?;
        let limit = args.limit.unwrap_or(8);
        if !(1..=20).contains(&limit) {
            return Err(invalid("limit must be between 1 and 20"));
        }
        let scope = scout_scope(args.repo)?;
        if let Some(search) = &args.search {
            search
                .validate(&scope)
                .map_err(|error| invalid(error.to_string()))?;
        }
        let config = self.session_config(args.profile)?;
        let engine = RecommendationEngine::new(&self.paths, &config);
        let options = ScoutOptions {
            include_filtered: args.include_filtered,
            record_exposure: args.record_exposure.unwrap_or(true),
            source: RecommendationEventSource::ToolScout,
        };
        let result = if let Some(search) = &args.search {
            engine
                .scout_search(limit, args.refresh, options, scope, search)
                .await?
        } else {
            engine.scout(limit, args.refresh, options, scope).await?
        };
        let candidates = result
            .ranked
            .iter()
            .map(|candidate| {
                let mut value = to_value(candidate_output(candidate));
                value["bodyExcerpt"] =
                    json!(candidate.issue.body.chars().take(700).collect::<String>());
                value["bodyTruncated"] = json!(candidate.issue.body.chars().count() > 700);
                value["warnings"] = json!(candidate.enriched_issue.warnings);
                value["prepareGate"] = to_value(prepare_gate_output(&candidate.value_assessment));
                value
            })
            .collect::<Vec<_>>();
        let partial = !result.diagnostics.stage_errors.is_empty()
            || !result.api_budget.budget_exhausted.is_empty()
            || result
                .ranked
                .iter()
                .any(|candidate| !candidate.enriched_issue.warnings.is_empty());
        let status = if partial {
            "partial"
        } else if candidates.is_empty() {
            "no_candidates"
        } else {
            "ok"
        };
        Ok(output(
            invocation,
            status,
            json!({
                "candidates":candidates,"discoveryCount":result.discovery_count,"filteredCount":result.filtered_count,
                "diagnostics":result.diagnostics,"apiBudget":result.api_budget,
                "profile":{"techStack":config.profile.tech_stack,"keywords":config.profile.keywords},
                "nextAction":"Inspect candidates with assess. If insufficient, refine search/profile or use diagnostics.search.nextPage; do not infer no competition from partial evidence."
            }),
            true,
        ))
    }

    async fn session_assess(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: AssessArgs = parse_arguments(&invocation.arguments)?;
        let reference = valid_issue(args.issue, args.url)?;
        let page = args.comments_page.unwrap_or(1);
        let per_page = args.comments_per_page.unwrap_or(30);
        if !(1..=100_000).contains(&page) || !(1..=100).contains(&per_page) {
            return Err(invalid("invalid comment pagination"));
        }
        let config = self.session_config(args.profile)?;
        let github = GitHubClient::new(&config)?;
        let context = github.issue_context(&reference, page, per_page).await?;
        if let Some(reason) = unavailable_reason(&context) {
            return Ok(output(
                invocation,
                "issue_unavailable",
                json!({"issue":context,"reason":reason}),
                true,
            ));
        }
        let assessment = workflow::assess_issue_selection_with_options(
            &self.paths,
            &config,
            IssueSelector::new(None, Some(context.url.clone())),
            args.refresh,
            args.record_read.unwrap_or(true),
            RecommendationEventSource::ToolAssess,
        )
        .await;
        let ranked = match assessment {
            Ok(ranked) => ranked,
            Err(error) => {
                return Ok(output(
                    invocation,
                    "partial",
                    json!({
                        "issue":context,"assessmentError":error.to_string(),
                        "nextAction":"The discussion is available but assessment could not finish. Inspect the error before selecting or preparing this issue."
                    }),
                    true,
                ))
            }
        };
        let partial = !ranked.enriched_issue.warnings.is_empty();
        Ok(output(
            invocation,
            if partial { "partial" } else { "ok" },
            json!({
                "issue":context,"assessment":assessment_output(&ranked),"prepareGate":prepare_gate_output(&ranked.value_assessment),
                "warnings":ranked.enriched_issue.warnings,"competition":ranked.enriched_issue.competition,
                "repository":ranked.enriched_issue.repository,"activity":ranked.enriched_issue.activity,
                "assessmentFetchedAt":ranked.enriched_issue.source_fetched_at,
                "nextAction":"Read all relevant comment pages and assess feasibility. Prepare only when implementation is in scope."
            }),
            true,
        ))
    }

    async fn session_prepare(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: PrepareArgs = parse_arguments(&invocation.arguments)?;
        let reference = valid_issue(args.issue, args.url)?;
        let bypass_reason = normalized_optional(args.bypass_reason);
        if args.allow_gate_bypass && bypass_reason.is_none() {
            return Err(invalid("allowGateBypass requires a nonempty bypassReason"));
        }
        let checkout = match args.checkout {
            Some(path) => path,
            None => {
                std::env::current_dir().map_err(|error| RuntimeFailure::System(error.into()))?
            }
        };
        let workspace_root = args
            .workspace_root
            .unwrap_or_else(|| default_workspace_root(&self.paths));
        absolute_path(&checkout)?;
        absolute_path(&workspace_root)?;
        let config = self.session_config(args.profile)?;
        let context = GitHubClient::new(&config)?
            .issue_context(&reference, 1, 30)
            .await?;
        if let Some(reason) = unavailable_reason(&context) {
            return Ok(output(
                invocation,
                "issue_unavailable",
                json!({"issue":context,"reason":reason}),
                true,
            ));
        }
        let ranked = workflow::assess_issue_selection_with_options(
            &self.paths,
            &config,
            IssueSelector::new(None, Some(context.url.clone())),
            true,
            true,
            RecommendationEventSource::ToolPrepare,
        )
        .await?;
        let decision = prepare_gate_decision(
            &ranked.value_assessment,
            if args.allow_gate_bypass {
                bypass_reason.as_deref()
            } else {
                None
            },
        );
        if let PrepareGateDecision::Blocked { .. } = decision {
            return Ok(output(
                invocation,
                "blocked_by_gate",
                json!({"issue":context,"assessment":assessment_output(&ranked),"prepareGate":prepare_gate_output(&ranked.value_assessment)}),
                true,
            ));
        }
        let evidence = json!({"issueContext":context,"assessment":assessment_output(&ranked),"competition":ranked.enriched_issue.competition,"warnings":ranked.enriched_issue.warnings,"gateBypass":gate_bypass_output(&decision)});
        let paths = self.paths.clone();
        let task = tokio::task::spawn_blocking(move || {
            crate::session::prepare(
                &paths,
                &ranked.issue,
                evidence,
                &crate::session::PrepareOptions {
                    checkout,
                    workspace_root,
                },
            )
        })
        .await
        .map_err(|error| RuntimeFailure::System(error.into()))??;
        Ok(output(invocation, "prepared", to_value(task), true))
    }
}

fn unavailable_reason(context: &IssueContext) -> Option<&'static str> {
    if context.is_pull_request {
        Some("The supplied reference is a pull request.")
    } else if context.state != "open" {
        Some("The issue is not open.")
    } else if context.locked {
        Some("The issue discussion is locked.")
    } else if !context.assignees.is_empty() {
        Some("The issue already has an assignee; inspect existing ownership before proceeding.")
    } else {
        None
    }
}

fn valid_issue(issue: Option<String>, url: Option<String>) -> RuntimeResult<IssueRef> {
    let reference = issue_selector(issue, url)?
        .issue_ref()
        .map_err(|error| invalid(error.to_string()))?;
    crate::workspace::validate_repository_name(&reference.repo_full_name())
        .map_err(|error| invalid(error.to_string()))?;
    if reference.number == 0 {
        return Err(invalid("issue number must be positive"));
    }
    Ok(reference)
}

fn absolute_path(path: &std::path::Path) -> RuntimeResult<()> {
    if !path.is_absolute() {
        return Err(invalid("workspace paths must be absolute"));
    }
    Ok(())
}

fn default_workspace_root(paths: &IssueFinderPaths) -> PathBuf {
    std::env::var_os("ISSUE_FINDER_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.workspaces_dir.clone())
}

fn invalid(message: impl Into<String>) -> RuntimeFailure {
    RuntimeFailure::InvalidArguments(message.into())
}

fn output(
    invocation: &IssueFinderToolInvocation,
    status: &str,
    mut data: Value,
    success: bool,
) -> IssueFinderToolOutput {
    data["kind"] = json!("issue_finder_tool_output");
    data["tool"] = json!(invocation.tool_name);
    data["status"] = json!(status);
    data["success"] = json!(success);
    data["sessionContractVersion"] = json!(1);
    if success {
        IssueFinderToolOutput::success(invocation, status, status, data)
    } else {
        IssueFinderToolOutput::failure_with_structured(invocation, status, status, data)
    }
}
