use std::path::Path;

use issue_finder::memory::{
    DispatchMemoryOutcome, GitHubInteractionMemoryEvent, ManualMemoryEvent, MemoryIngestor,
    MemoryNodeType, MemoryRawEventType, MemoryRole, MemorySourceType, MemoryStore,
    MemorySubjectType, MemoryTrustLevel,
};
use issue_finder::paths::IssueFinderPaths;
use issue_finder::profile_bootstrap::{
    ActiveProject, AgentSourceReport, BootstrapWarning, EvidenceOutput, ProfileBootstrapReport,
    RecentTaskTheme, RecommendedProfile, ScanScope,
};
use issue_finder::recommendation::{
    IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};
use serde_json::json;
use tempfile::tempdir;

const NOW: &str = "2026-06-18T00:00:00Z";

#[test]
fn recommendation_feedback_ingests_raw_event_and_node_idempotently() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let event = RecommendationEvent {
        event_id: "recommendation-event-1".to_string(),
        timestamp: NOW.to_string(),
        issue_key: IssueKey::new("owner/repo", 77),
        event_type: RecommendationEventType::Dismissed,
        source: RecommendationEventSource::FeedbackCommand,
        issue_updated_at: Some("2026-06-17T00:00:00Z".to_string()),
        issue_comments_count: Some(4),
        metadata: json!({"reason": "too broad"}),
    };

    let first = ingestor.ingest_recommendation_event(&event).unwrap();
    let second = ingestor.ingest_recommendation_event(&event).unwrap();

    assert_eq!(first, second);
    assert_eq!(store.list_raw_events().unwrap().len(), 1);
    let source = store.get_source(&first.source_id).unwrap().unwrap();
    assert_eq!(source.source_type, MemorySourceType::RecommendationEvent);
    assert_eq!(source.source_ref, "recommendation-event-1");
    assert_eq!(source.trust_level, MemoryTrustLevel::UserExplicit);

    let raw = store
        .get_raw_event(&first.raw_event_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(raw.event_type, MemoryRawEventType::Dismiss);
    assert_eq!(raw.role, MemoryRole::User);
    assert_eq!(raw.trust_level, MemoryTrustLevel::UserExplicit);
    assert_eq!(raw.subject_type, MemorySubjectType::Issue);
    assert_eq!(raw.subject_ref, "owner/repo#77");
    assert_eq!(raw.payload_json["metadata"]["reason"], "too broad");

    let nodes = store.list_nodes_for_raw_event(&raw.id).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].node_type, MemoryNodeType::RawEvent);
    assert_eq!(
        nodes[0].text_ref,
        Some(format!("memory_raw_events:{}", raw.id))
    );
}

#[test]
fn dispatch_failure_ingests_agent_task_failure_and_validation_nodes() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let outcome = DispatchMemoryOutcome {
        id: "dispatch-run-1".to_string(),
        issue_key: IssueKey::new("owner/repo", 42),
        agent_id: "codex".to_string(),
        task_type: "rust_cli_panic".to_string(),
        succeeded: false,
        failure_reason: Some("unclear_validation".to_string()),
        validation_paths: vec!["cargo test -p cli".to_string()],
        artifact_refs: vec!["inbox/item/handoff.json".to_string()],
        occurred_at: NOW.to_string(),
        metadata: json!({"workspace": "/tmp/work"}),
    };

    let result = ingestor.ingest_dispatch_outcome(&outcome).unwrap();
    ingestor.ingest_dispatch_outcome(&outcome).unwrap();

    assert_eq!(store.list_raw_events().unwrap().len(), 1);
    let source = store.get_source(&result.source_id).unwrap().unwrap();
    assert_eq!(source.source_type, MemorySourceType::DispatchEvent);
    assert_eq!(source.source_ref, "dispatch-run-1");
    assert_eq!(source.trust_level, MemoryTrustLevel::AgentObserved);

    let raw = store
        .get_raw_event(&result.raw_event_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(raw.event_type, MemoryRawEventType::DispatchFailure);
    assert_eq!(raw.role, MemoryRole::Agent);
    assert_eq!(raw.subject_ref, "owner/repo#42");

    let nodes = store.list_nodes_for_raw_event(&raw.id).unwrap();
    assert_eq!(nodes.len(), 5);
    assert!(nodes
        .iter()
        .any(|node| node.node_type == MemoryNodeType::RawEvent));
    assert!(nodes.iter().any(|node| {
        node.entity_type.as_deref() == Some("agent")
            && node.normalized_value.as_deref() == Some("codex")
    }));
    assert!(nodes.iter().any(|node| {
        node.entity_type.as_deref() == Some("task_type")
            && node.normalized_value.as_deref() == Some("rust_cli_panic")
    }));
    assert!(nodes.iter().any(|node| {
        node.entity_type.as_deref() == Some("failure_reason")
            && node.normalized_value.as_deref() == Some("unclear_validation")
    }));
    assert!(nodes.iter().any(|node| {
        node.entity_type.as_deref() == Some("validation_path")
            && node.normalized_value.as_deref() == Some("cargo test -p cli")
    }));
}

#[test]
fn profile_bootstrap_evidence_ingests_source_refs_and_report_terms() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);
    let report = profile_report();

    let first = ingestor
        .ingest_profile_bootstrap_report(&report, "profile-bootstrap-2026-06-18", NOW)
        .unwrap();
    let second = ingestor
        .ingest_profile_bootstrap_report(&report, "profile-bootstrap-2026-06-18", NOW)
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(store.list_raw_events().unwrap().len(), 3);
    let source = store.get_source(&first.source_id).unwrap().unwrap();
    assert_eq!(source.source_type, MemorySourceType::ProfileBootstrap);
    assert_eq!(source.source_ref, "profile-bootstrap-2026-06-18");
    assert_eq!(source.trust_level, MemoryTrustLevel::SystemObserved);

    let events = store.list_raw_events().unwrap();
    assert!(events
        .iter()
        .all(|event| event.source_id == first.source_id));
    assert!(events.iter().all(|event| event.role == MemoryRole::System));
    assert!(events
        .iter()
        .any(|event| event.payload_json["term"] == "Rust"));
    assert!(events
        .iter()
        .any(|event| event.payload_json["term"] == "tokio"));
    assert!(events
        .iter()
        .any(|event| event.payload_json["theme"] == "CLI parser fixes"));
}

#[test]
fn manual_and_github_ingestion_preserve_authority_boundaries() {
    let dir = tempdir().unwrap();
    let paths = test_paths(dir.path());
    let store = MemoryStore::open(&paths).unwrap();
    let ingestor = MemoryIngestor::new(&store);

    let manual = ingestor
        .ingest_manual_event(&ManualMemoryEvent {
            id: "manual-1".to_string(),
            event_type: MemoryRawEventType::Reject,
            role: MemoryRole::User,
            trust_level: MemoryTrustLevel::UserExplicit,
            subject_type: MemorySubjectType::Repo,
            subject_ref: "owner/repo".to_string(),
            payload_json: json!({"reason": "avoid broad refactors"}),
            occurred_at: NOW.to_string(),
        })
        .unwrap();
    let github = ingestor
        .ingest_github_interaction(&GitHubInteractionMemoryEvent {
            id: "github-comment-1".to_string(),
            repo_full_name: "owner/repo".to_string(),
            issue_number: Some(9),
            maintainer: Some("maintainer".to_string()),
            interaction_type: "maintainer_reply".to_string(),
            occurred_at: NOW.to_string(),
            payload_json: json!({"comment": "please add tests"}),
        })
        .unwrap();

    let manual_raw = store
        .get_raw_event(&manual.raw_event_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(manual_raw.role, MemoryRole::User);
    assert_eq!(manual_raw.trust_level, MemoryTrustLevel::UserExplicit);

    let github_raw = store
        .get_raw_event(&github.raw_event_ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(github_raw.role, MemoryRole::Github);
    assert_eq!(github_raw.trust_level, MemoryTrustLevel::ExternalGithub);
    assert_eq!(github_raw.subject_ref, "owner/repo#9");
    let github_nodes = store.list_nodes_for_raw_event(&github_raw.id).unwrap();
    assert!(github_nodes.iter().any(|node| {
        node.entity_type.as_deref() == Some("maintainer")
            && node.normalized_value.as_deref() == Some("maintainer")
    }));
}

fn profile_report() -> ProfileBootstrapReport {
    ProfileBootstrapReport {
        kind: "issue_finder_profile_bootstrap_report".to_string(),
        version: 1,
        scan_scope: ScanScope {
            agent_sources: vec!["codex".to_string()],
            scan_depth: "root_manifest_only".to_string(),
            full_supported_source_scan: true,
            conversation_body_mode: "disabled".to_string(),
        },
        agent_sources: vec![AgentSourceReport {
            kind: "codex".to_string(),
            path: "/tmp/.codex/history.jsonl".to_string(),
            status: "ok".to_string(),
            records_seen: 1,
            records_parsed: 1,
            warnings: Vec::<BootstrapWarning>::new(),
        }],
        active_projects: vec![ActiveProject {
            id: "project-1".to_string(),
            path: "/tmp/project".to_string(),
            first_seen_at: Some(NOW.to_string()),
            last_seen_at: Some(NOW.to_string()),
            session_count: 1,
            memory_count: 0,
            manifest_count: 1,
            sources: vec!["codex:history".to_string()],
            manifests: vec!["/tmp/project/Cargo.toml".to_string()],
        }],
        tech_stack_evidence: vec![EvidenceOutput {
            term: "Rust".to_string(),
            weight: 42,
            count: 2,
            sources: vec!["codex:history".to_string()],
            project_refs: vec!["project-1".to_string()],
            manifest_refs: vec!["/tmp/project/Cargo.toml".to_string()],
            reason: None,
        }],
        keyword_evidence: vec![EvidenceOutput {
            term: "tokio".to_string(),
            weight: 15,
            count: 1,
            sources: vec!["manifest:Cargo.toml".to_string()],
            project_refs: vec!["project-1".to_string()],
            manifest_refs: vec!["/tmp/project/Cargo.toml".to_string()],
            reason: Some("dependency".to_string()),
        }],
        recent_task_themes: vec![RecentTaskTheme {
            theme: "CLI parser fixes".to_string(),
            count: 1,
            sources: vec!["codex:history".to_string()],
            last_seen_at: Some(NOW.to_string()),
        }],
        recommended_profile: RecommendedProfile {
            tech_stack: vec!["Rust".to_string()],
            keywords: vec!["tokio".to_string()],
        },
        warnings: Vec::<BootstrapWarning>::new(),
    }
}

fn test_paths(root: &Path) -> IssueFinderPaths {
    IssueFinderPaths {
        home: root.to_path_buf(),
        config: root.join("config.toml"),
        cache_dir: root.join("cache"),
        workspaces_dir: root.join("workspaces"),
        inbox_dir: root.join("inbox"),
        reports_dir: root.join("reports"),
    }
}
