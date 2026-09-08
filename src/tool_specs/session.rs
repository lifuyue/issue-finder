use super::*;

pub(super) fn list_session_specs() -> IssueFinderToolSpecsEnvelope {
    let profile = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "techStack": {"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":100}},
            "keywords": {"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":100}}
        },
        "description": "Override only this invocation's recommendation preferences; never writes config."
    });
    let mut scout = scout_schema();
    scout["properties"]["limit"]["maximum"] = json!(20);
    scout["properties"]["limit"]["default"] = json!(8);
    scout["properties"]["profile"] = profile.clone();
    scout["properties"]["search"] = json!({
        "type":"object", "additionalProperties":false,
        "description":"Bounded GitHub search followed by local value/competition ranking. Omit for the curated recommendation feed. No unrelated fallback for explicit search.",
        "properties": {
            "query":{"type":"string","maxLength":1024,"default":"","description":"GitHub search terms/qualifiers; open issue and optional repository scope are enforced."},
            "sort":{"type":"string","enum":["best_match","created","updated","comments"],"default":"updated"},
            "order":{"type":"string","enum":["asc","desc"],"default":"desc"},
            "page":{"type":"integer","minimum":1,"maximum":1000,"default":1},
            "perPage":{"type":"integer","minimum":1,"maximum":100,"default":30},
            "maxPages":{"type":"integer","minimum":1,"maximum":10,"default":2},
            "apiBudget":{"type":"integer","minimum":1,"maximum":1200,"default":120}
        }
    });
    let mut assess = assess_schema();
    assess["properties"]["profile"] = profile.clone();
    assess["properties"]["commentsPage"] =
        json!({"type":"integer","minimum":1,"maximum":100000,"default":1});
    assess["properties"]["commentsPerPage"] =
        json!({"type":"integer","minimum":1,"maximum":100,"default":30});
    let mut prepare = prepare_schema();
    prepare["properties"]
        .as_object_mut()
        .unwrap()
        .remove("refresh");
    prepare["properties"]["profile"] = profile;
    prepare["properties"]["checkout"] = json!({"type":"string","description":"Absolute checkout to inspect for reuse. Defaults to current directory. Only reused when its remote matches the selected issue repository."});
    prepare["properties"]["workspaceRoot"] = json!({"type":"string","description":"Absolute contribution root; defaults to ISSUE_FINDER_WORKSPACE_ROOT or the CLI workspace directory."});
    let workspace =
        json!({"type":"string","description":"Absolute workspace returned by prepare."});
    IssueFinderToolSpecsEnvelope {
        kind: "issue_finder_tool_specs".into(), version: 1, session_contract_version: Some(1),
        quick_start: ToolQuickStart {
            summary: "The current agent owns user interaction, selection, coding and review. CLI tools provide discovery, evidence, workspace preparation and verified result collection without dispatch or extra approval objects.".into(),
            first_call: ToolFirstCall {
                default_tool: TOOL_SCOUT.into(), default_arguments: json!({"limit":8}),
                when_ready_unknown: TOOL_STATUS.into(), fallback_after_setup_failure: TOOL_STATUS.into(),
            },
        },
        recommended_workflow: vec![
            workflow_step("discover", TOOL_SCOUT, "Refine search/profile using diagnostics and bounded pagination. Omit repo for global discovery; the tool project is not implicitly the target."),
            workflow_step("assess", TOOL_ASSESS, "Read issue body, complete paged comments and competition evidence. Select in the current session."),
            workflow_step("prepare", TOOL_PREPARE, "Recheck live issue and gate, then prepare one isolated task. Stop here for prepare-only requests."),
            workflow_step("resume", TOOL_TASK_STATUS, "Inspect task/workspace identity and changes; continue coding and reviewing in the current agent."),
            workflow_step("finish", TOOL_FINISH, "Run explicit argv checks, collect baseline changes and persist the result. Review final diff before reporting success."),
        ],
        tools: vec![
            tool_spec("status", "Check version, configuration and GitHub authentication; missing config is supported and no interactive init is needed.", status_schema(), false),
            tool_spec("scout", "Discover and rank issues. Returns explanations, body excerpts, API budget, partial evidence and search continuation. GitHub retrieval order differs from final local ranking.", scout, false),
            tool_spec("assess", "Read full issue body and one page of full comment bodies, plus value/gate/competition evidence. No workspace/handoff is created. Follow nextCommentsPage until discussion is complete.", assess, false),
            tool_spec("prepare", "Prepare the selected issue in the current session, without dispatch/handoff/approval state. Always refresh evidence. A gate bypass needs an explicit reason, never automatic retry.", prepare, false),
            tool_spec("task_status", "Validate and resume a prepared task using its immutable baseline; includes local changes and last result.", json!({"type":"object","properties":{"workspace":workspace.clone()},"required":["workspace"],"additionalProperties":false}), false),
            tool_spec("finish", "Run agent-selected checks without a shell, validate task identity, collect all changes and persist completion feedback. Failed checks retain recoverable state. Checks execute repository code under host permissions.", json!({
                "type":"object","additionalProperties":false,"required":["workspace","checks"],
                "properties": {
                    "workspace":workspace,
                    "checks":{"type":"array","minItems":1,"maxItems":20,"items":{"type":"array","minItems":1,"maxItems":100,"items":{"type":"string","maxLength":32768}}},
                    "checkTimeoutSeconds":{"type":"integer","minimum":1,"maximum":3600,"default":600},
                    "summary":{"type":"string","maxLength":32768,"description":"Agent's description of the change; not a CLI certification of semantic correctness."}
                }
            }), false),
            tool_spec("feedback", "Record read, dismiss or restore feedback using recommendation state. Does not post to GitHub or mark unverified work complete.", json!({"type":"object","properties":{"issue":{"type":"string"},"action":{"type":"string","enum":["read","dismiss","restore"]}},"required":["issue","action"],"additionalProperties":false}), false),
        ],
    }
}
