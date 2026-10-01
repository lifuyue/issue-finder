use super::*;

pub(super) fn list_session_specs() -> IssueFinderToolSpecsEnvelope {
    let profile = json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "techStack": {"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":100}},
            "keywords": {"type":"array","maxItems":20,"items":{"type":"string","minLength":1,"maxLength":100}},
            "taskPreferences": {"type":"string","maxLength":4000,"description":"Explicit task inclusion/exclusion preferences for this call, such as avoiding documentation polishing. Omission inherits configured preferences."}
        },
        "description": "Override only this invocation's recommendation preferences; never writes config."
    });
    let mut scout = scout_schema();
    scout["properties"]["limit"]["maximum"] = json!(20);
    scout["properties"]["limit"]["default"] = json!(8);
    scout["properties"]["profile"] = profile.clone();
    scout["properties"]["search"] = json!({
        "type":"object", "additionalProperties":false,
        "description":"Bounded GitHub search with fresh initial availability checks, seven v2 System 1 questions, deterministic ranking and a fresh final availability recheck/backfill within the same budget. Omit for the curated recommendation feed. No unrelated fallback for explicit search.",
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
            summary: "Discover and assess GitHub issues for Codex. Codex owns selection, workspace preparation, reproduction, implementation, verification, review and PR delivery. Scout uses seven v2 System 1 questions via Codex app-server (gpt-6-luna, reasoning none), one process and independent issue threads with default concurrency 4. Fresh initial/final GitHub availability checks are independent of semantic caching; assess performs final-depth checks without a model request. The environment configures both CLIs and authentication; business calls disclose configuration, access and semantic failures.".into(),
            first_call: ToolFirstCall {
                default_tool: TOOL_SCOUT.into(), default_arguments: json!({"limit":8}),
                when_ready_unknown: TOOL_SCOUT.into(), fallback_after_setup_failure: TOOL_SCOUT.into(),
            },
        },
        recommended_workflow: vec![
            workflow_step("discover", TOOL_SCOUT, "Refine search/profile using diagnostics and bounded pagination. Inspect seven v2 System 1 answers, fresh availability coverage and PR relations, incomplete materials, isolated failures and budget/fact skips. Omit repo for global discovery; the tool project is not implicitly the target."),
            workflow_step("assess", TOOL_ASSESS, "Read fresh issue body, paged comments and final-depth GitHub availability evidence without a model request; old scout screening is historical. Distinguish explicit resolving PRs from mentions/search leads and closed from merged, then inspect current code where needed. Recommendation factors inform selection; Codex decides whether to proceed."),
        ],
        tools: vec![
            tool_spec("scout", "Discover and rank issues with fresh initial GitHub availability checks, one request per issue containing seven v2 semantic questions with each question's material, and fresh final checks with backfill within the bounded pool/API budget. Default concurrency is 4; independent issue threads share one app-server process and isolate candidate failures, with at most one retry for a retryable server response. Returns availability coverage and PR identities/relations, typed answers, snapshots, execution diagnostics and partial failures. Discussion work/fix claims are soft evidence-check reminders; concrete task forms are governed by goal, clarity, scope and explicit preferences. Six-hour semantic caching never replaces fresh facts. Historical eight-question replays preserve their recorded outcome. GitHub retrieval order differs from final ranking.", scout, false),
            tool_spec("assess", "Read fresh full issue body, one page of full comments and final-depth GitHub availability facts without a model request. Reports issue/assignment/archive/lock state, linked/search coverage and verified PR identities, state, merge/base-branch facts and relations. Explicit resolving references differ from mentions/search leads; closed does not imply merged and merged evidence does not prove a fix. Missing evidence remains unknown. Old scout answers remain historical screening. Follow nextCommentsPage when discussion matters. Recommendation scores do not authorize or block repair.", assess, false),
        ],
    }
}
