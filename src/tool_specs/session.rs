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
    IssueFinderToolSpecsEnvelope {
        kind: "issue_finder_tool_specs".into(), version: 1, session_contract_version: Some(2),
        quick_start: ToolQuickStart {
            summary: "Discover and assess GitHub issues for Codex. Codex owns selection, workspace preparation, reproduction, implementation, verification, review and PR delivery. The environment configures installation, dependencies, PATH and authentication; business calls return configuration and access errors directly.".into(),
            first_call: ToolFirstCall {
                default_tool: TOOL_SCOUT.into(), default_arguments: json!({"limit":8}),
                when_ready_unknown: TOOL_SCOUT.into(), fallback_after_setup_failure: TOOL_SCOUT.into(),
            },
        },
        recommended_workflow: vec![
            workflow_step("discover", TOOL_SCOUT, "Refine search/profile using diagnostics and bounded pagination. Omit repo for global discovery; the tool project is not implicitly the target."),
            workflow_step("assess", TOOL_ASSESS, "Read issue body, paged comments and competition evidence. Recommendation factors inform selection; Codex decides whether to proceed."),
        ],
        tools: vec![
            tool_spec("scout", "Discover and rank issues. Returns explanations, body excerpts, API budget, partial evidence and search continuation. GitHub retrieval order differs from final local ranking.", scout, false),
            tool_spec("assess", "Read full issue body and one page of full comment bodies, plus value, repository and competition evidence. Follow nextCommentsPage until discussion is complete. Recommendation scores do not authorize or block repair.", assess, false),
        ],
    }
}
