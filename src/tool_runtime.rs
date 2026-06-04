use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::github::GitHubIssue;
use crate::inbox;
use crate::paths::PatchbayPaths;
use crate::report::PreparedReportItem;
use crate::value_scoring::{
    GateStatus, GateVerdict, RankedValueIssue, RecommendationCategory, ValueAssessment,
};
use crate::workflow::{self, PrepareOptions, PrepareOutcome};

const TOOL_SCOUT: &str = "patchbay.scout";
const TOOL_ASSESS: &str = "patchbay.assess";
const TOOL_PREPARE: &str = "patchbay.prepare";
const TOOL_READ_CONTEXT: &str = "patchbay.read_context";
const DEFAULT_CONTEXT_MAX_BYTES: usize = 12_000;
const CONTEXT_MAX_BYTES_LIMIT: usize = 50_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatchbayToolSpecsEnvelope {
    pub kind: String,
    pub version: u8,
    pub tools: Vec<PatchbayToolSpec>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PatchbayToolSpec {
    pub namespace: Option<String>,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub defer_loading: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatchbayToolInvocation {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PatchbayToolOutput {
    pub call_id: String,
    pub turn_id: Option<String>,
    pub tool_name: String,
    pub success: bool,
    pub status: String,
    pub content_items: Vec<PatchbayContentItem>,
    pub structured_content: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PatchbayContentItem {
    InputText { text: String },
}

#[derive(Debug, Clone)]
pub struct PatchbayToolRuntime {
    paths: PatchbayPaths,
    config: Config,
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

impl PatchbayToolInvocation {
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

impl PatchbayToolOutput {
    fn success(
        invocation: &PatchbayToolInvocation,
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
            content_items: vec![PatchbayContentItem::InputText {
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
            content_items: vec![PatchbayContentItem::InputText {
                text: message.clone(),
            }],
            structured_content: json!({
                "kind": "patchbay_tool_output",
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
        invocation: &PatchbayToolInvocation,
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
            content_items: vec![PatchbayContentItem::InputText {
                text: content_text.into(),
            }],
            structured_content,
        }
    }
}

impl PatchbayToolRuntime {
    pub fn new(paths: PatchbayPaths, config: Config) -> Self {
        Self { paths, config }
    }

    pub async fn execute(&self, invocation: PatchbayToolInvocation) -> PatchbayToolOutput {
        if !invocation.arguments.is_object() {
            return PatchbayToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "invalid_arguments",
                "arguments must be a JSON object",
            );
        }

        let result = match invocation.tool_name.as_str() {
            TOOL_SCOUT => self.call_scout(&invocation).await,
            TOOL_ASSESS => self.call_assess(&invocation).await,
            TOOL_PREPARE => self.call_prepare(&invocation).await,
            TOOL_READ_CONTEXT => self.call_read_context(&invocation),
            _ => Err(RuntimeFailure::InvalidArguments(format!(
                "unknown Patchbay tool {}",
                invocation.tool_name
            ))),
        };

        match result {
            Ok(output) => output,
            Err(RuntimeFailure::InvalidArguments(message)) => PatchbayToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "invalid_arguments",
                message,
            ),
            Err(RuntimeFailure::System(error)) => PatchbayToolOutput::failure(
                invocation.call_id,
                invocation.turn_id,
                invocation.tool_name,
                "system_error",
                error.to_string(),
            ),
        }
    }

    async fn call_scout(
        &self,
        invocation: &PatchbayToolInvocation,
    ) -> RuntimeResult<PatchbayToolOutput> {
        let args: ScoutToolArgs = parse_arguments(&invocation.arguments)?;
        let limit = args.limit.unwrap_or(10).max(1);
        let _reserved_min_category = args.min_category;
        let ranked = workflow::scout(&self.paths, &self.config, limit.max(25), args.refresh)
            .await
            .map_err(RuntimeFailure::System)?;
        let filtered_count = ranked
            .iter()
            .filter(|candidate| {
                candidate.value_assessment.recommendation_category
                    == RecommendationCategory::FilteredLowDepth
            })
            .count();
        let candidates = ranked
            .iter()
            .filter(|candidate| {
                args.include_filtered
                    || candidate.value_assessment.recommendation_category
                        != RecommendationCategory::FilteredLowDepth
            })
            .take(limit)
            .map(candidate_json)
            .collect::<Vec<_>>();
        let structured = json!({
            "kind": "patchbay_tool_output",
            "tool": TOOL_SCOUT,
            "status": "ok",
            "success": true,
            "candidates": candidates,
            "filteredCount": filtered_count,
        });
        Ok(PatchbayToolOutput::success(
            invocation,
            "ok",
            format!(
                "Found {} candidates ({} filtered).",
                candidates.len(),
                filtered_count
            ),
            structured,
        ))
    }

    async fn call_assess(
        &self,
        invocation: &PatchbayToolInvocation,
    ) -> RuntimeResult<PatchbayToolOutput> {
        let args: AssessToolArgs = parse_arguments(&invocation.arguments)?;
        let (issue, url) = issue_selector(args.issue, args.url)?;
        let ranked =
            workflow::assess_from_input(&self.paths, &self.config, issue, url, args.refresh)
                .await
                .map_err(RuntimeFailure::System)?;
        let issue_label = issue_label(&ranked.issue);
        let structured = json!({
            "kind": "patchbay_tool_output",
            "tool": TOOL_ASSESS,
            "status": "ok",
            "success": true,
            "issue": issue_json(&ranked.issue),
            "assessment": assessment_json(&ranked),
            "prepareGate": prepare_gate_json(&ranked.value_assessment),
        });
        Ok(PatchbayToolOutput::success(
            invocation,
            "ok",
            format!(
                "Assessed {issue_label}: {}.",
                ranked.value_assessment.recommendation_category
            ),
            structured,
        ))
    }

    async fn call_prepare(
        &self,
        invocation: &PatchbayToolInvocation,
    ) -> RuntimeResult<PatchbayToolOutput> {
        let args: PrepareToolArgs = parse_arguments(&invocation.arguments)?;
        let bypass_reason = normalized_optional(args.bypass_reason);
        if args.allow_gate_bypass && bypass_reason.is_none() {
            return Err(RuntimeFailure::InvalidArguments(
                "allowGateBypass=true requires a non-empty bypassReason".to_string(),
            ));
        }

        let (issue, url) = issue_selector(args.issue, args.url)?;
        let ranked =
            workflow::assess_from_input(&self.paths, &self.config, issue, url, args.refresh)
                .await
                .map_err(RuntimeFailure::System)?;
        let category = ranked.value_assessment.recommendation_category;
        if !prepare_default_allowed(category) {
            let reasons = prepare_gate_reasons(&ranked.value_assessment);
            if !args.allow_gate_bypass {
                let structured = json!({
                    "kind": "patchbay_tool_output",
                    "tool": TOOL_PREPARE,
                    "status": "blocked_by_gate",
                    "success": true,
                    "issue": issue_json(&ranked.issue),
                    "assessment": assessment_json(&ranked),
                    "prepareGate": blocked_prepare_gate_json(category, reasons),
                });
                return Ok(PatchbayToolOutput::success(
                    invocation,
                    "blocked_by_gate",
                    format!(
                        "Prepare blocked by gate for {}: {}.",
                        issue_label(&ranked.issue),
                        category
                    ),
                    structured,
                ));
            }
        }

        let gate_bypass = if prepare_default_allowed(category) {
            None
        } else {
            Some((
                category,
                bypass_reason.expect("bypass reason was validated above"),
            ))
        };
        let issue = ranked.issue.clone();
        let assessment = ranked.value_assessment.clone();
        let assessment_payload = assessment_json(&ranked);
        let outcome = workflow::prepare_value_issue_with_options(
            &self.paths,
            &self.config,
            ranked,
            PrepareOptions {
                explicit_prepare: true,
                gate_bypass_reason: gate_bypass
                    .as_ref()
                    .map(|(_category, reason)| reason.clone()),
            },
        )
        .await
        .map_err(RuntimeFailure::System)?;

        Ok(prepare_outcome_output(
            invocation,
            &self.paths,
            &issue,
            &assessment,
            assessment_payload,
            outcome,
            gate_bypass,
        ))
    }

    fn call_read_context(
        &self,
        invocation: &PatchbayToolInvocation,
    ) -> RuntimeResult<PatchbayToolOutput> {
        let args: ReadContextToolArgs = parse_arguments(&invocation.arguments)?;
        let section_path = section_relative_path(&args.section).ok_or_else(|| {
            RuntimeFailure::InvalidArguments(format!(
                "unsupported context section {}",
                args.section
            ))
        })?;
        let item =
            inbox::find_item(&self.paths, &args.handoff_id).map_err(RuntimeFailure::System)?;
        let handoff_json_path = PathBuf::from(&item.handoff_json_path);
        let handoff_dir = handoff_json_path
            .parent()
            .context("inbox item has no handoff directory")
            .map_err(RuntimeFailure::System)?;
        let handoff_dir = canonicalize_existing(handoff_dir)?;
        let target = canonicalize_existing(&handoff_dir.join(section_path))?;
        if !target.starts_with(&handoff_dir) {
            return Err(RuntimeFailure::InvalidArguments(
                "context section resolves outside the handoff directory".to_string(),
            ));
        }

        let max_bytes = args
            .max_bytes
            .unwrap_or(DEFAULT_CONTEXT_MAX_BYTES)
            .min(CONTEXT_MAX_BYTES_LIMIT);
        let bytes = fs::read(&target)
            .with_context(|| format!("unable to read {}", target.display()))
            .map_err(RuntimeFailure::System)?;
        let truncated = bytes.len() > max_bytes;
        let visible_bytes = if truncated {
            &bytes[..max_bytes]
        } else {
            &bytes[..]
        };
        let content = String::from_utf8_lossy(visible_bytes).to_string();
        let structured = json!({
            "kind": "patchbay_tool_output",
            "tool": TOOL_READ_CONTEXT,
            "status": "ok",
            "success": true,
            "handoffId": args.handoff_id,
            "section": args.section,
            "path": target.to_string_lossy(),
            "truncated": truncated,
            "content": content,
        });
        Ok(PatchbayToolOutput::success(
            invocation,
            "ok",
            "Read context section.",
            structured,
        ))
    }
}

pub fn list_tool_specs() -> PatchbayToolSpecsEnvelope {
    PatchbayToolSpecsEnvelope {
        kind: "patchbay_tool_specs".to_string(),
        version: 1,
        tools: vec![
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
                "Read one fixed section from a prepared Patchbay handoff context pack.",
                read_context_schema(),
                true,
            ),
        ],
    }
}

pub fn default_call_id() -> String {
    format!("patchbay-call-{}", Utc::now().timestamp_millis())
}

fn prepare_outcome_output(
    invocation: &PatchbayToolInvocation,
    paths: &PatchbayPaths,
    issue: &GitHubIssue,
    assessment: &ValueAssessment,
    assessment_payload: Value,
    outcome: PrepareOutcome,
    gate_bypass: Option<(RecommendationCategory, String)>,
) -> PatchbayToolOutput {
    match outcome {
        PrepareOutcome::Prepared(item) => prepared_output(
            invocation,
            paths,
            issue,
            assessment,
            assessment_payload,
            &item,
            gate_bypass,
        ),
        PrepareOutcome::Failed(item) => {
            let structured = json!({
                "kind": "patchbay_tool_output",
                "tool": TOOL_PREPARE,
                "status": "prepare_failed",
                "success": false,
                "issue": issue_json(issue),
                "assessment": assessment_payload,
                "prepareGate": prepare_gate_json(assessment),
                "failure": {
                    "repoFullName": item.repo_full_name,
                    "issueNumber": item.issue_number,
                    "reason": item.reason,
                },
                "gateBypass": gate_bypass_json(gate_bypass.as_ref()),
            });
            PatchbayToolOutput::failure_with_structured(
                invocation,
                "prepare_failed",
                "Preparation failed.",
                structured,
            )
        }
    }
}

fn prepared_output(
    invocation: &PatchbayToolInvocation,
    paths: &PatchbayPaths,
    issue: &GitHubIssue,
    assessment: &ValueAssessment,
    assessment_payload: Value,
    item: &PreparedReportItem,
    gate_bypass: Option<(RecommendationCategory, String)>,
) -> PatchbayToolOutput {
    let dir = PathBuf::from(&item.handoff_json_path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|| paths.inbox_item_dir(&item.id).to_string_lossy().to_string());
    let structured = json!({
        "kind": "patchbay_tool_output",
        "tool": TOOL_PREPARE,
        "status": "prepared",
        "success": true,
        "issue": issue_json(issue),
        "assessment": assessment_payload,
        "prepareGate": prepare_gate_json(assessment),
        "handoff": {
            "id": item.id,
            "dir": dir,
            "handoffJsonPath": item.handoff_json_path,
            "handoffMarkdownPath": item.handoff_md_path,
            "codexMarkdownPath": item.codex_md_path,
            "agentPolicyPath": item.agent_policy_path,
            "probeJsonPath": item.probe_json_path,
            "prepareEventsPath": item.prepare_events_path,
        },
        "readiness": {
            "score": item.readiness_score,
            "band": item.readiness_band,
        },
        "gateBypass": gate_bypass_json(gate_bypass.as_ref()),
    });
    PatchbayToolOutput::success(
        invocation,
        "prepared",
        format!("Prepared {}.", item.id),
        structured,
    )
}

fn candidate_json(candidate: &RankedValueIssue) -> Value {
    json!({
        "issue": issue_json(&candidate.issue),
        "category": candidate.value_assessment.recommendation_category.to_string(),
        "rankScore": candidate.value_assessment.final_rank_score,
        "scores": scores_json(&candidate.value_assessment),
        "gates": gates_json(&candidate.value_assessment),
        "riskTags": risk_tags_json(&candidate.value_assessment),
        "missingEvidence": candidate.value_assessment.missing_evidence,
    })
}

fn assessment_json(candidate: &RankedValueIssue) -> Value {
    json!({
        "category": candidate.value_assessment.recommendation_category.to_string(),
        "rankScore": candidate.value_assessment.final_rank_score,
        "gates": gates_json(&candidate.value_assessment),
        "scores": scores_json(&candidate.value_assessment),
        "riskTags": risk_tags_json(&candidate.value_assessment),
        "missingEvidence": candidate.value_assessment.missing_evidence,
        "competition": {
            "openPrRefs": candidate.enriched_issue.competition.open_pr_refs,
            "closedPrRefs": candidate.enriched_issue.competition.closed_pr_refs,
            "attemptComments": candidate.enriched_issue.competition.attempt_comments,
            "claimComments": candidate.enriched_issue.competition.claim_comments,
            "workingComments": candidate.enriched_issue.competition.working_comments,
            "fixSubmittedComments": candidate.enriched_issue.competition.fix_submitted_comments,
            "competitionPoints": candidate.enriched_issue.competition.competition_points,
            "competitionBand": candidate.enriched_issue.competition.competition_band.to_string(),
            "warnings": candidate.enriched_issue.competition.warnings,
        }
    })
}

fn issue_json(issue: &GitHubIssue) -> Value {
    json!({
        "repoFullName": issue.repo_full_name,
        "number": issue.number,
        "title": issue.title,
        "url": issue.url,
    })
}

fn issue_label(issue: &GitHubIssue) -> String {
    format!("{}#{}", issue.repo_full_name, issue.number)
}

fn scores_json(assessment: &ValueAssessment) -> Value {
    json!({
        "repoInfluence": assessment.scores.repo_influence_score,
        "profileFit": assessment.scores.profile_fit_score,
        "executionQuality": assessment.scores.execution_quality_score,
        "maintainerSignal": assessment.scores.maintainer_signal_score,
        "freshness": assessment.scores.freshness_score,
        "risk": assessment.scores.risk_score,
    })
}

fn gates_json(assessment: &ValueAssessment) -> Value {
    json!({
        "lowDepth": gate_json(&assessment.gates.low_depth),
        "repoInfluence": gate_json(&assessment.gates.repo_influence),
        "competition": gate_json(&assessment.gates.competition),
        "profileFit": gate_json(&assessment.gates.profile_fit),
    })
}

fn gate_json(gate: &GateVerdict) -> Value {
    json!({
        "status": gate.status.to_string(),
        "band": gate.band.to_string(),
        "reasons": gate.reasons,
        "evidenceRefs": gate.evidence_refs,
    })
}

fn risk_tags_json(assessment: &ValueAssessment) -> Vec<String> {
    assessment
        .risk_tags
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn prepare_gate_json(assessment: &ValueAssessment) -> Value {
    let category = assessment.recommendation_category;
    if prepare_default_allowed(category) {
        json!({
            "defaultAllowed": true,
            "allowedCategories": allowed_prepare_categories(),
            "requiresBypass": false,
            "reasons": [],
        })
    } else {
        blocked_prepare_gate_json(category, prepare_gate_reasons(assessment))
    }
}

fn blocked_prepare_gate_json(category: RecommendationCategory, reasons: Vec<String>) -> Value {
    json!({
        "defaultAllowed": false,
        "allowedCategories": allowed_prepare_categories(),
        "requiresBypass": true,
        "blockedCategory": category.to_string(),
        "reasons": reasons,
        "bypassAvailable": true,
    })
}

fn gate_bypass_json(gate_bypass: Option<&(RecommendationCategory, String)>) -> Value {
    match gate_bypass {
        Some((category, reason)) => json!({
            "allowed": true,
            "reason": reason,
            "originalBlockedCategory": category.to_string(),
        }),
        None => Value::Null,
    }
}

fn prepare_default_allowed(category: RecommendationCategory) -> bool {
    matches!(
        category,
        RecommendationCategory::HighValueReady | RecommendationCategory::HighValueNeedsScoping
    )
}

fn allowed_prepare_categories() -> Vec<String> {
    [
        RecommendationCategory::HighValueReady,
        RecommendationCategory::HighValueNeedsScoping,
    ]
    .into_iter()
    .map(|category| category.to_string())
    .collect()
}

fn prepare_gate_reasons(assessment: &ValueAssessment) -> Vec<String> {
    let mut reasons = Vec::new();
    collect_gate_reasons(&mut reasons, &assessment.gates.low_depth);
    collect_gate_reasons(&mut reasons, &assessment.gates.repo_influence);
    collect_gate_reasons(&mut reasons, &assessment.gates.competition);
    collect_gate_reasons(&mut reasons, &assessment.gates.profile_fit);
    if assessment.execution_score < 50 {
        reasons.push(format!(
            "Execution score is below prepare threshold ({})",
            assessment.execution_score
        ));
    }
    for tag in &assessment.risk_tags {
        reasons.push(format!("Risk tag: {tag}"));
    }
    for item in &assessment.missing_evidence {
        reasons.push(format!("Missing evidence: {item}"));
    }
    if reasons.is_empty() {
        reasons.push(format!(
            "Category {} is outside the default prepare gate",
            assessment.recommendation_category
        ));
    }
    reasons.sort();
    reasons.dedup();
    reasons
}

fn collect_gate_reasons(reasons: &mut Vec<String>, gate: &GateVerdict) {
    if gate.status != GateStatus::Pass {
        reasons.extend(gate.reasons.clone());
    }
}

fn section_relative_path(section: &str) -> Option<&'static Path> {
    match section {
        "entry" => Some(Path::new("context/entry.md")),
        "safety" => Some(Path::new("context/safety.md")),
        "probe" => Some(Path::new("context/probe.md")),
        "value" => Some(Path::new("context/value.md")),
        "issue" => Some(Path::new("context/issue.md")),
        "repo" => Some(Path::new("context/repo.md")),
        "validation" => Some(Path::new("context/validation.md")),
        "handoff_json" => Some(Path::new("handoff.json")),
        "agent_policy" => Some(Path::new("agent-policy.json")),
        "probe_json" => Some(Path::new("probe.json")),
        _ => None,
    }
}

fn parse_arguments<T>(arguments: &Value) -> RuntimeResult<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments.clone())
        .map_err(|error| RuntimeFailure::InvalidArguments(error.to_string()))
}

fn issue_selector(
    issue: Option<String>,
    url: Option<String>,
) -> RuntimeResult<(Option<String>, Option<String>)> {
    let issue = normalized_optional(issue);
    let url = normalized_optional(url);
    match (issue, url) {
        (Some(issue), None) => Ok((Some(issue), None)),
        (None, Some(url)) => Ok((None, Some(url))),
        (Some(_), Some(_)) => Err(RuntimeFailure::InvalidArguments(
            "pass either issue or url, not both".to_string(),
        )),
        (None, None) => Err(RuntimeFailure::InvalidArguments(
            "pass issue or url".to_string(),
        )),
    }
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn canonicalize_existing(path: &Path) -> RuntimeResult<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("unable to resolve {}", path.display()))
        .map_err(RuntimeFailure::System)
}

fn tool_spec(
    name: &str,
    description: &str,
    input_schema: Value,
    defer_loading: bool,
) -> PatchbayToolSpec {
    PatchbayToolSpec {
        namespace: Some("patchbay".to_string()),
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        defer_loading,
    }
}

fn scout_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": { "type": "integer", "minimum": 1, "default": 10 },
            "refresh": { "type": "boolean", "default": false },
            "includeFiltered": { "type": "boolean", "default": false },
            "minCategory": { "type": ["string", "null"], "default": null }
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
            "refresh": { "type": "boolean", "default": false }
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
#[serde(rename_all = "camelCase")]
struct ScoutToolArgs {
    limit: Option<usize>,
    #[serde(default)]
    refresh: bool,
    #[serde(default)]
    include_filtered: bool,
    #[serde(default)]
    min_category: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssessToolArgs {
    #[serde(default)]
    issue: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    refresh: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadContextToolArgs {
    handoff_id: String,
    section: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::{
        list_tool_specs, PatchbayToolInvocation, TOOL_ASSESS, TOOL_PREPARE, TOOL_READ_CONTEXT,
        TOOL_SCOUT,
    };

    #[test]
    fn lists_four_patchbay_tool_specs() {
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
            vec![TOOL_SCOUT, TOOL_ASSESS, TOOL_PREPARE, TOOL_READ_CONTEXT]
        );
    }

    #[test]
    fn invocation_requires_json_object_arguments() {
        let error = PatchbayToolInvocation::from_json_arguments(
            TOOL_SCOUT.to_string(),
            "[]",
            Some("call_1".to_string()),
            None,
        )
        .unwrap_err();
        assert!(error.contains("JSON object"));
    }
}
