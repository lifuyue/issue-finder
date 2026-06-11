use std::path::PathBuf;

use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{Config, GitHubTokenSource};
use crate::github::{GitHubClient, GitHubIssue};
use crate::paths::IssueFinderPaths;
use crate::prepare_gate::{prepare_gate_decision, PrepareGateDecision};
use crate::recommendation::{
    DiscoveryScope, RecommendationEventSource, RepositoryScope, ScoutOptions,
};
use crate::tool_context::{read_context_section, ReadContextError, ReadContextToolArgs};
use crate::tool_outputs::{
    assess_structured_output, assessment_output, candidate_output, failure_output,
    gate_bypass_output, handoff_output, issue_output, prepare_blocked_structured_output,
    prepare_failed_structured_output, prepare_gate_output, prepare_prepared_structured_output,
    readiness_output, scout_structured_output, status_structured_output, to_value,
    AssessmentOutput, GateBypassOutput, IssueOutput, PrepareGateOutput, StatusConfigOutput,
    StatusGitHubAuthOutput, StatusGitHubOutput,
};
use crate::value_scoring::RankedValueIssue;
use crate::workflow::{self, IssueSelector, PrepareOptions, PrepareOutcome};

const TOOL_SCOUT: &str = "issue-finder.scout";
const TOOL_ASSESS: &str = "issue-finder.assess";
const TOOL_PREPARE: &str = "issue-finder.prepare";
const TOOL_READ_CONTEXT: &str = "issue-finder.read_context";
const TOOL_STATUS: &str = "issue-finder.status";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IssueFinderToolSpecsEnvelope {
    pub kind: String,
    pub version: u8,
    pub tools: Vec<IssueFinderToolSpec>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IssueFinderToolSpec {
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub defer_loading: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueFinderToolInvocation {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IssueFinderToolOutput {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub success: bool,
    pub status: String,
    pub content_items: Vec<IssueFinderContentItem>,
    pub structured_content: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IssueFinderContentItem {
    InputText { text: String },
}

#[derive(Debug, Clone)]
pub struct IssueFinderToolRuntime {
    paths: IssueFinderPaths,
    config: Config,
    config_load_error: Option<String>,
}

#[derive(Debug)]
enum RuntimeFailure {
    InvalidArguments(String),
    System(anyhow::Error),
}

type RuntimeResult<T> = std::result::Result<T, RuntimeFailure>;

impl From<anyhow::Error> for RuntimeFailure {
    fn from(error: anyhow::Error) -> Self {
        Self::System(error)
    }
}

impl From<ReadContextError> for RuntimeFailure {
    fn from(error: ReadContextError) -> Self {
        match error {
            ReadContextError::InvalidArguments(message) => Self::InvalidArguments(message),
            ReadContextError::System(error) => Self::System(error),
        }
    }
}

impl IssueFinderToolInvocation {
    pub fn from_json_arguments(
        tool_name: String,
        arguments: &str,
        call_id: Option<String>,
        turn_id: Option<String>,
    ) -> std::result::Result<Self, String> {
        let arguments = serde_json::from_str::<Value>(arguments)
            .map_err(|error| format!("arguments must be valid JSON: {error}"))?;
        if !arguments.is_object() {
            return Err("arguments must be a JSON object".to_string());
        }

        Ok(Self {
            call_id: call_id.unwrap_or_else(default_call_id),
            turn_id,
            tool_name,
            arguments,
        })
    }
}

impl IssueFinderToolOutput {
    fn success(
        invocation: &IssueFinderToolInvocation,
        status: impl Into<String>,
        content_text: impl Into<String>,
        structured_content: Value,
    ) -> Self {
        Self {
            call_id: invocation.call_id.clone(),
            turn_id: invocation.turn_id.clone(),
            tool_name: invocation.tool_name.clone(),
            success: true,
            status: status.into(),
            content_items: vec![IssueFinderContentItem::InputText {
                text: content_text.into(),
            }],
            structured_content,
        }
    }

    pub fn failure(
        call_id: impl Into<String>,
        turn_id: Option<String>,
        tool_name: impl Into<String>,
        status: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let tool_name = tool_name.into();
        let status = status.into();
        let message = message.into();
        Self {
            call_id: call_id.into(),
            turn_id,
            tool_name: tool_name.clone(),
            success: false,
            status: status.clone(),
            content_items: vec![IssueFinderContentItem::InputText {
                text: message.clone(),
            }],
            structured_content: json!({
                "kind": "issue_finder_tool_output",
                "tool": tool_name,
                "status": status,
                "success": false,
                "error": {
                    "message": message
                }
            }),
        }
    }

    fn failure_with_structured(
        invocation: &IssueFinderToolInvocation,
        status: impl Into<String>,
        content_text: impl Into<String>,
        structured_content: Value,
    ) -> Self {
        Self {
            call_id: invocation.call_id.clone(),
            turn_id: invocation.turn_id.clone(),
            tool_name: invocation.tool_name.clone(),
            success: false,
            status: status.into(),
            content_items: vec![IssueFinderContentItem::InputText {
                text: content_text.into(),
            }],
            structured_content,
        }
    }
}

impl IssueFinderToolRuntime {
    pub fn new(paths: IssueFinderPaths, config: Config) -> Self {
        Self {
            paths,
            config,
            config_load_error: None,
        }
    }

    pub fn new_with_config_load_error(
        paths: IssueFinderPaths,
        config: Config,
        config_load_error: Option<String>,
    ) -> Self {
        Self {
            paths,
            config,
            config_load_error,
        }
    }

    pub async fn execute(&self, invocation: IssueFinderToolInvocation) -> IssueFinderToolOutput {
        if !invocation.arguments.is_object() {
            return IssueFinderToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "invalid_arguments",
                "arguments must be a JSON object",
            );
        }

        let result = match invocation.tool_name.as_str() {
            TOOL_STATUS => self.call_status(&invocation).await,
            TOOL_SCOUT => self.call_scout(&invocation).await,
            TOOL_ASSESS => self.call_assess(&invocation).await,
            TOOL_PREPARE => self.call_prepare(&invocation).await,
            TOOL_READ_CONTEXT => self.call_read_context(&invocation),
            _ => Err(RuntimeFailure::InvalidArguments(format!(
                "unknown Issue Finder tool {}",
                invocation.tool_name
            ))),
        };

        match result {
            Ok(output) => output,
            Err(RuntimeFailure::InvalidArguments(message)) => IssueFinderToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "invalid_arguments",
                message,
            ),
            Err(RuntimeFailure::System(error)) => IssueFinderToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "system_error",
                error.to_string(),
            ),
        }
    }

    async fn call_status(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: StatusToolArgs = parse_arguments(&invocation.arguments)?;
        let check_auth = args.check_auth.unwrap_or(true);
        let config_exists = self.paths.config.exists();
        let config_load_ok = !config_exists || self.config_load_error.is_none();
        let token = self.config.resolved_github_token();
        let mut auth = StatusGitHubAuthOutput {
            checked: check_auth,
            ok: false,
            login: None,
            error: None,
        };

        if check_auth {
            if token.source == GitHubTokenSource::Missing {
                auth.error = Some("GitHub token is missing".to_string());
            } else {
                match GitHubClient::new(&self.config) {
                    Ok(client) => match client.validate_token().await {
                        Ok(login) => {
                            auth.ok = true;
                            auth.login = Some(login);
                        }
                        Err(error) => {
                            auth.error = Some(error.to_string());
                        }
                    },
                    Err(error) => {
                        auth.error = Some(error.to_string());
                    }
                }
            }
        }

        let status = status_name(
            config_exists,
            config_load_ok,
            token.source,
            check_auth.then_some(auth.ok),
        );
        let next_fix_command = next_fix_command(
            config_exists,
            config_load_ok,
            token.source,
            check_auth,
            auth.ok,
        );
        let content_text = status_content_text(&status, token.source, &auth, &next_fix_command);
        let structured = status_structured_output(
            TOOL_STATUS,
            status.clone(),
            StatusConfigOutput {
                path: self.paths.config.to_string_lossy().to_string(),
                exists: config_exists,
                load_ok: config_load_ok,
                load_error: self.config_load_error.clone(),
            },
            StatusGitHubOutput {
                token_source: token.source.as_str().to_string(),
                auth,
            },
            next_fix_command,
        );

        Ok(IssueFinderToolOutput::success(
            invocation,
            status,
            content_text,
            structured,
        ))
    }

    async fn call_scout(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: ScoutToolArgs = parse_arguments(&invocation.arguments)?;
        let limit = args.limit.unwrap_or(10).max(1);
        let scope = scout_scope(args.repo)?;
        let result = workflow::scout_with_options(
            &self.paths,
            &self.config,
            limit,
            args.refresh,
            ScoutOptions {
                include_filtered: args.include_filtered,
                record_exposure: args.record_exposure.unwrap_or(true),
                source: RecommendationEventSource::ToolScout,
            },
            scope,
        )
        .await
        .map_err(RuntimeFailure::System)?;
        let candidates = result
            .ranked
            .iter()
            .take(limit)
            .map(candidate_output)
            .collect::<Vec<_>>();
        let candidate_count = candidates.len();
        Ok(IssueFinderToolOutput::success(
            invocation,
            "ok",
            format!(
                "Found {candidate_count} candidates ({} filtered).",
                result.filtered_count
            ),
            scout_structured_output(
                TOOL_SCOUT,
                candidates,
                result.filtered_count,
                result.diagnostics,
            ),
        ))
    }

    async fn call_assess(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: AssessToolArgs = parse_arguments(&invocation.arguments)?;
        let selector = issue_selector(args.issue, args.url)?;
        let ranked = self
            .assess_selection(
                selector,
                args.refresh,
                args.record_read.unwrap_or(true),
                RecommendationEventSource::ToolAssess,
            )
            .await?;
        let issue_label = issue_label(&ranked.issue);
        let issue = issue_output(&ranked.issue);
        let assessment = assessment_output(&ranked);
        let prepare_gate = prepare_gate_output(&ranked.value_assessment);
        Ok(IssueFinderToolOutput::success(
            invocation,
            "ok",
            format!(
                "Assessed {issue_label}: {}.",
                ranked.value_assessment.recommendation_category
            ),
            assess_structured_output(TOOL_ASSESS, issue, assessment, prepare_gate),
        ))
    }

    async fn call_prepare(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: PrepareToolArgs = parse_arguments(&invocation.arguments)?;
        let bypass_reason = normalized_optional(args.bypass_reason);
        if args.allow_gate_bypass && bypass_reason.is_none() {
            return Err(RuntimeFailure::InvalidArguments(
                "allowGateBypass=true requires a non-empty bypassReason".to_string(),
            ));
        }

        let selector = issue_selector(args.issue, args.url)?;
        let ranked = self
            .assess_selection(
                selector,
                args.refresh,
                true,
                RecommendationEventSource::ToolPrepare,
            )
            .await?;
        let issue = issue_output(&ranked.issue);
        let assessment = assessment_output(&ranked);
        let prepare_gate = prepare_gate_output(&ranked.value_assessment);
        let decision = prepare_gate_decision(
            &ranked.value_assessment,
            args.allow_gate_bypass
                .then_some(bypass_reason.as_deref())
                .flatten(),
        );

        if let PrepareGateDecision::Blocked { .. } = &decision {
            let structured =
                prepare_blocked_structured_output(TOOL_PREPARE, issue, assessment, prepare_gate);
            return Ok(IssueFinderToolOutput::success(
                invocation,
                "blocked_by_gate",
                format!(
                    "Prepare blocked by gate for {}: {}.",
                    issue_label(&ranked.issue),
                    ranked.value_assessment.recommendation_category
                ),
                structured,
            ));
        }

        let prepare_output = PrepareOutputParts {
            issue_label: issue_label(&ranked.issue),
            issue,
            assessment,
            prepare_gate,
        };
        let gate_bypass = gate_bypass_output(&decision);
        let outcome = workflow::prepare_value_issue_with_options(
            &self.paths,
            &self.config,
            ranked,
            PrepareOptions {
                explicit_prepare: true,
                gate_bypass_reason: bypass_reason_for_prepare(&decision),
                recommendation_source: Some(RecommendationEventSource::ToolPrepare),
            },
        )
        .await
        .map_err(RuntimeFailure::System)?;

        Ok(prepare_outcome_output(
            invocation,
            &self.paths,
            prepare_output,
            outcome,
            gate_bypass,
        ))
    }

    fn call_read_context(
        &self,
        invocation: &IssueFinderToolInvocation,
    ) -> RuntimeResult<IssueFinderToolOutput> {
        let args: ReadContextToolArgs = parse_arguments(&invocation.arguments)?;
        let structured = read_context_section(&self.paths, TOOL_READ_CONTEXT, args)?;
        Ok(IssueFinderToolOutput::success(
            invocation,
            "ok",
            "Read context section.",
            to_value(structured),
        ))
    }

    async fn assess_selection(
        &self,
        selector: IssueSelector,
        refresh: bool,
        record_read: bool,
        source: RecommendationEventSource,
    ) -> RuntimeResult<RankedValueIssue> {
        workflow::assess_issue_selection_with_options(
            &self.paths,
            &self.config,
            selector,
            refresh,
            record_read,
            source,
        )
        .await
        .map_err(RuntimeFailure::System)
    }
}

pub fn list_tool_specs() -> IssueFinderToolSpecsEnvelope {
    IssueFinderToolSpecsEnvelope {
        kind: "issue_finder_tool_specs".to_string(),
        version: 1,
        tools: vec![
            tool_spec(
                "status",
                "Report Issue Finder config, GitHub token source, and auth readiness without exposing tokens.",
                status_schema(),
                false,
            ),
            tool_spec(
                "scout",
                "Discover and rank candidate GitHub issues with gate-aware summaries.",
                scout_schema(),
                false,
            ),
            tool_spec(
                "assess",
                "Assess one GitHub issue without preparing workspace or handoff state.",
                assess_schema(),
                false,
            ),
            tool_spec(
                "prepare",
                "Prepare a workspace and handoff for one issue after the prepare gate passes.",
                prepare_schema(),
                false,
            ),
            tool_spec(
                "read_context",
                "Read one fixed section from a prepared Issue Finder handoff context pack.",
                read_context_schema(),
                true,
            ),
        ],
    }
}

pub fn default_call_id() -> String {
    format!("issue-finder-call-{}", Utc::now().timestamp_millis())
}

fn prepare_outcome_output(
    invocation: &IssueFinderToolInvocation,
    paths: &IssueFinderPaths,
    output: PrepareOutputParts,
    outcome: PrepareOutcome,
    gate_bypass: Option<GateBypassOutput>,
) -> IssueFinderToolOutput {
    match outcome {
        PrepareOutcome::Prepared(item) => {
            let dir = PathBuf::from(&item.handoff_json_path)
                .parent()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| paths.inbox_item_dir(&item.id).to_string_lossy().to_string());
            let structured = prepare_prepared_structured_output(
                TOOL_PREPARE,
                output.issue,
                output.assessment,
                output.prepare_gate,
                handoff_output(&item, dir),
                readiness_output(&item),
                gate_bypass,
            );
            IssueFinderToolOutput::success(
                invocation,
                "prepared",
                format!("Prepared {}.", item.id),
                structured,
            )
        }
        PrepareOutcome::Failed(item) => {
            let structured = prepare_failed_structured_output(
                TOOL_PREPARE,
                output.issue,
                output.assessment,
                output.prepare_gate,
                failure_output(&item),
                gate_bypass,
            );
            IssueFinderToolOutput::failure_with_structured(
                invocation,
                "prepare_failed",
                format!(
                    "Preparation failed for {}: {}.",
                    output.issue_label, item.reason
                ),
                structured,
            )
        }
    }
}

struct PrepareOutputParts {
    issue_label: String,
    issue: IssueOutput,
    assessment: AssessmentOutput,
    prepare_gate: PrepareGateOutput,
}

fn bypass_reason_for_prepare(decision: &PrepareGateDecision) -> Option<String> {
    match decision {
        PrepareGateDecision::Bypassed { reason, .. } => Some(reason.clone()),
        PrepareGateDecision::Allowed | PrepareGateDecision::Blocked { .. } => None,
    }
}

fn issue_selector(issue: Option<String>, url: Option<String>) -> RuntimeResult<IssueSelector> {
    let selector = IssueSelector::new(normalized_optional(issue), normalized_optional(url));
    selector
        .issue_ref()
        .map_err(|error| RuntimeFailure::InvalidArguments(error.to_string()))?;
    Ok(selector)
}

fn scout_scope(repo: Option<String>) -> RuntimeResult<DiscoveryScope> {
    match normalized_optional(repo) {
        Some(repo) => RepositoryScope::parse(&repo)
            .map(DiscoveryScope::repository)
            .map_err(|error| RuntimeFailure::InvalidArguments(error.to_string())),
        None => Ok(DiscoveryScope::Global),
    }
}

fn status_name(
    config_exists: bool,
    config_load_ok: bool,
    token_source: GitHubTokenSource,
    auth_ok: Option<bool>,
) -> String {
    if token_source == GitHubTokenSource::Missing || !config_exists || !config_load_ok {
        return "needs_setup".to_string();
    }

    if auth_ok == Some(false) {
        return "auth_failed".to_string();
    }

    "ready".to_string()
}

fn next_fix_command(
    config_exists: bool,
    config_load_ok: bool,
    token_source: GitHubTokenSource,
    check_auth: bool,
    auth_ok: bool,
) -> Option<String> {
    if !config_load_ok {
        return Some("issue-finder init --force".to_string());
    }

    if token_source == GitHubTokenSource::Missing || (check_auth && !auth_ok) {
        return Some(r#"export GITHUB_TOKEN="$(gh auth token)""#.to_string());
    }

    if !config_exists {
        return Some("issue-finder init".to_string());
    }

    None
}

fn status_content_text(
    status: &str,
    token_source: GitHubTokenSource,
    auth: &StatusGitHubAuthOutput,
    next_fix_command: &Option<String>,
) -> String {
    let auth_text = if auth.checked {
        if auth.ok {
            format!(
                "GitHub authenticated as {} using {}.",
                auth.login.as_deref().unwrap_or("unknown"),
                token_source.as_str()
            )
        } else {
            format!(
                "GitHub auth is not available: {}.",
                auth.error.as_deref().unwrap_or("unknown error")
            )
        }
    } else {
        format!(
            "GitHub auth was not checked; token source is {}.",
            token_source.as_str()
        )
    };

    match next_fix_command {
        Some(command) => {
            format!("Issue Finder status: {status}. {auth_text} Next fix: `{command}`.")
        }
        None => format!("Issue Finder status: {status}. {auth_text}"),
    }
}

fn issue_label(issue: &GitHubIssue) -> String {
    format!("{}#{}", issue.repo_full_name, issue.number)
}

fn parse_arguments<T>(arguments: &Value) -> RuntimeResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments.clone())
        .map_err(|error| RuntimeFailure::InvalidArguments(error.to_string()))
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn tool_spec(
    name: &str,
    description: &str,
    input_schema: Value,
    defer_loading: bool,
) -> IssueFinderToolSpec {
    IssueFinderToolSpec {
        namespace: Some("issue-finder".to_string()),
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        defer_loading,
    }
}

fn status_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "checkAuth": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    })
}

fn scout_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "default": 10 },
            "repo": { "type": ["string", "null"], "default": null },
            "refresh": { "type": "boolean", "default": false },
            "includeFiltered": { "type": "boolean", "default": false },
            "recordExposure": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    })
}

fn assess_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": ["string", "null"] },
            "url": { "type": ["string", "null"] },
            "refresh": { "type": "boolean", "default": false },
            "recordRead": { "type": "boolean", "default": true }
        },
        "additionalProperties": false
    })
}

fn prepare_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "issue": { "type": ["string", "null"] },
            "url": { "type": ["string", "null"] },
            "refresh": { "type": "boolean", "default": false },
            "allowGateBypass": { "type": "boolean", "default": false },
            "bypassReason": { "type": ["string", "null"], "default": null }
        },
        "additionalProperties": false
    })
}

fn read_context_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handoffId": { "type": "string" },
            "section": {
                "type": "string",
                "enum": [
                    "entry",
                    "safety",
                    "probe",
                    "value",
                    "issue",
                    "repo",
                    "validation",
                    "handoff_json",
                    "agent_policy",
                    "probe_json"
                ]
            },
            "maxBytes": {
                "type": "integer",
                "minimum": 0,
                "maximum": 50000,
                "default": 12000
            }
        },
        "required": ["handoffId", "section"],
        "additionalProperties": false
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StatusToolArgs {
    #[serde(default)]
    check_auth: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScoutToolArgs {
    limit: Option<usize>,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    include_filtered: bool,
    #[serde(default)]
    record_exposure: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssessToolArgs {
    #[serde(default)]
    issue: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    record_read: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrepareToolArgs {
    #[serde(default)]
    issue: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    allow_gate_bypass: bool,
    #[serde(default)]
    bypass_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        list_tool_specs, IssueFinderToolInvocation, TOOL_ASSESS, TOOL_PREPARE, TOOL_READ_CONTEXT,
        TOOL_SCOUT, TOOL_STATUS,
    };

    #[test]
    fn lists_five_issue_finder_tool_specs() {
        let specs = list_tool_specs();
        let names = specs
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "{}.{}",
                    tool.namespace.as_deref().unwrap_or_default(),
                    tool.name
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                TOOL_STATUS,
                TOOL_SCOUT,
                TOOL_ASSESS,
                TOOL_PREPARE,
                TOOL_READ_CONTEXT
            ]
        );
    }

    #[test]
    fn invocation_requires_json_object_arguments() {
        let error = IssueFinderToolInvocation::from_json_arguments(
            TOOL_SCOUT.to_string(),
            "[]",
            Some("call_1".to_string()),
            None,
        )
        .unwrap_err();
        assert!(error.contains("JSON object"));
    }
}
