use super::*;
use crate::discovery::SearchOptions;
use crate::github::{IssueContext, IssueRef};
use crate::recommendation::RecommendationEngine;

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

impl IssueFinderToolRuntime {
    pub(super) async fn call_session_tool(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        match invocation.tool_name.as_str() {
            TOOL_SCOUT => self.session_scout(invocation).await,
            TOOL_ASSESS => self.session_assess(invocation).await,
            _ => Err(invalid("tool is not part of the Codex discovery workflow")),
        }
    }

    fn session_config(&self, profile: ProfileArgs) -> RuntimeResult<Config> {
        if let Some(error) = &self.config_load_error {
            return Err(RuntimeFailure::System(anyhow::anyhow!(
                "unable to load configuration {}: {error}",
                self.paths.config.display()
            )));
        }
        let mut config = self.config.clone();
        let token = config.resolved_session_github_token();
        if token.source == GitHubTokenSource::Missing {
            return Err(RuntimeFailure::System(anyhow::anyhow!(
                "GitHub authentication is unavailable; configure GITHUB_TOKEN or authenticate gh in this execution environment, then retry the business tool."
            )));
        }
        config.github.token = token.token;
        // Codex owns semantic review; discovery never invokes a second LLM.
        config.llm.enabled = false;
        profile.apply(&mut config)?;
        Ok(config)
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
        let engine = RecommendationEngine::for_codex(&self.paths, &config);
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
        let mut issue_warnings = Vec::new();
        if context.locked {
            issue_warnings.push("The issue discussion is locked.");
        }
        if !context.assignees.is_empty() {
            issue_warnings.push(
                "The issue already has an assignee; inspect existing ownership before proceeding.",
            );
        }
        let issue = github.fetch_issue_for_assessment(&reference).await?;
        let assessment = RecommendationEngine::for_codex(&self.paths, &config)
            .assess_issue(
                issue,
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
                        "issue":context,"issueWarnings":issue_warnings,"assessmentError":error.to_string(),
                        "nextAction":"The discussion is available but assessment could not finish. Inspect the error before selecting this issue."
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
                "issue":context,"issueWarnings":issue_warnings,"assessment":assessment_output(&ranked),
                "warnings":ranked.enriched_issue.warnings,"competition":ranked.enriched_issue.competition,
                "repository":ranked.enriched_issue.repository,"activity":ranked.enriched_issue.activity,
                "assessmentFetchedAt":ranked.enriched_issue.source_fetched_at,
                "nextAction":"Read all relevant comment pages and assess feasibility. Scores and recommendation factors inform selection; Codex owns reproduction, repair, verification and PR delivery."
            }),
            true,
        ))
    }
}

fn unavailable_reason(context: &IssueContext) -> Option<&'static str> {
    if context.is_pull_request {
        Some("The supplied reference is a pull request.")
    } else if context.state != "open" {
        Some("The issue is not open.")
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
    data["sessionContractVersion"] = json!(2);
    if success {
        IssueFinderToolOutput::success(invocation, status, status, data)
    } else {
        IssueFinderToolOutput::failure_with_structured(invocation, status, status, data)
    }
}
