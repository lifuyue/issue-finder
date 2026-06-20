use anyhow::Result;
use serde_json::{json, Value};

use crate::memory::model::*;
use crate::memory::store::MemoryStore;
use crate::profile_bootstrap::{EvidenceOutput, ProfileBootstrapReport, RecentTaskTheme};
use crate::recommendation::{
    IssueKey, RecommendationEvent, RecommendationEventSource, RecommendationEventType,
};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryIngestResult {
    pub source_id: String,
    pub raw_event_ids: Vec<String>,
    pub node_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchMemoryOutcome {
    pub id: String,
    pub issue_key: IssueKey,
    pub agent_id: String,
    pub outcome_kind: Option<String>,
    pub task_type: String,
    pub succeeded: bool,
    pub failure_class: Option<String>,
    pub failure_reason: Option<String>,
    pub validation_outcome: Option<String>,
    pub validation_paths: Vec<String>,
    pub artifact_refs: Vec<String>,
    pub occurred_at: String,
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GitHubInteractionMemoryEvent {
    pub id: String,
    pub repo_full_name: String,
    pub issue_number: Option<u64>,
    pub maintainer: Option<String>,
    pub interaction_type: String,
    pub occurred_at: String,
    pub payload_json: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManualMemoryEvent {
    pub id: String,
    pub event_type: MemoryRawEventType,
    pub role: MemoryRole,
    pub trust_level: MemoryTrustLevel,
    pub subject_type: MemorySubjectType,
    pub subject_ref: String,
    pub payload_json: Value,
    pub occurred_at: String,
}

pub struct MemoryIngestor<'a> {
    store: &'a MemoryStore,
}

impl<'a> MemoryIngestor<'a> {
    pub fn new(store: &'a MemoryStore) -> Self {
        Self { store }
    }

    pub fn ingest_recommendation_event(
        &self,
        event: &RecommendationEvent,
    ) -> Result<MemoryIngestResult> {
        let source_id = stable_id("memory-source-recommendation", &event.event_id);
        self.ensure_source(NewMemorySource {
            id: source_id.clone(),
            source_type: MemorySourceType::RecommendationEvent,
            source_ref: event.event_id.clone(),
            trust_level: recommendation_trust(event),
            created_at: event.timestamp.clone(),
        })?;

        let raw_id = stable_id("memory-raw-recommendation", &event.event_id);
        let payload = json!({
            "eventId": event.event_id,
            "eventType": event.event_type,
            "source": event.source,
            "issue": event.issue_key.label(),
            "issueUpdatedAt": event.issue_updated_at,
            "issueCommentsCount": event.issue_comments_count,
            "metadata": event.metadata,
        });
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.clone(),
            event_type: recommendation_event_type(event.event_type),
            role: recommendation_role(event),
            trust_level: recommendation_trust(event),
            subject_type: MemorySubjectType::Issue,
            subject_ref: event.issue_key.label(),
            payload_json: payload,
            confidence: recommendation_confidence(event),
            occurred_at: event.timestamp.clone(),
            created_at: event.timestamp.clone(),
        })?;

        let node_id = self.ensure_raw_event_node(
            "memory-node-recommendation",
            &raw_id,
            &event.timestamp,
            json!({
                "summary": format!(
                    "recommendation {:?} for {}",
                    event.event_type,
                    event.issue_key.label()
                )
            }),
        )?;
        Ok(MemoryIngestResult {
            source_id,
            raw_event_ids: vec![raw_id],
            node_ids: vec![node_id],
        })
    }

    pub fn ingest_dispatch_outcome(
        &self,
        outcome: &DispatchMemoryOutcome,
    ) -> Result<MemoryIngestResult> {
        let source_id = stable_id("memory-source-dispatch", &outcome.id);
        self.ensure_source(NewMemorySource {
            id: source_id.clone(),
            source_type: MemorySourceType::DispatchEvent,
            source_ref: outcome.id.clone(),
            trust_level: MemoryTrustLevel::AgentObserved,
            created_at: outcome.occurred_at.clone(),
        })?;

        let raw_id = stable_id("memory-raw-dispatch", &outcome.id);
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.clone(),
            event_type: if outcome.succeeded {
                MemoryRawEventType::DispatchSuccess
            } else {
                MemoryRawEventType::DispatchFailure
            },
            role: MemoryRole::Agent,
            trust_level: MemoryTrustLevel::AgentObserved,
            subject_type: MemorySubjectType::Issue,
            subject_ref: outcome.issue_key.label(),
            payload_json: json!({
                "outcomeId": outcome.id,
                "issue": outcome.issue_key.label(),
                "agentId": outcome.agent_id,
                "outcomeKind": outcome.outcome_kind,
                "taskType": outcome.task_type,
                "succeeded": outcome.succeeded,
                "failureClass": outcome.failure_class,
                "failureReason": outcome.failure_reason,
                "validationOutcome": outcome.validation_outcome,
                "validationPaths": outcome.validation_paths,
                "artifactRefs": outcome.artifact_refs,
                "metadata": outcome.metadata,
            }),
            confidence: 0.9,
            occurred_at: outcome.occurred_at.clone(),
            created_at: outcome.occurred_at.clone(),
        })?;

        let mut node_ids = vec![self.ensure_raw_event_node(
            "memory-node-dispatch",
            &raw_id,
            &outcome.occurred_at,
            json!({
                "summary": if outcome.succeeded {
                    "dispatch succeeded"
                } else {
                    "dispatch failed"
                }
            }),
        )?];
        node_ids.push(self.ensure_entity_node(
            &raw_id,
            "agent",
            &outcome.agent_id,
            &outcome.occurred_at,
        )?);
        node_ids.push(self.ensure_entity_node(
            &raw_id,
            "task_type",
            &outcome.task_type,
            &outcome.occurred_at,
        )?);
        if let Some(outcome_kind) = outcome.outcome_kind.as_deref() {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "outcome_kind",
                outcome_kind,
                &outcome.occurred_at,
            )?);
        }
        if let Some(failure_class) = outcome.failure_class.as_deref() {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "failure_class",
                failure_class,
                &outcome.occurred_at,
            )?);
        }
        if let Some(reason) = outcome.failure_reason.as_deref() {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "failure_reason",
                reason,
                &outcome.occurred_at,
            )?);
        }
        if let Some(validation_outcome) = outcome.validation_outcome.as_deref() {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "validation_outcome",
                validation_outcome,
                &outcome.occurred_at,
            )?);
        }
        for validation_path in &outcome.validation_paths {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "validation_path",
                validation_path,
                &outcome.occurred_at,
            )?);
        }

        Ok(MemoryIngestResult {
            source_id,
            raw_event_ids: vec![raw_id],
            node_ids,
        })
    }

    pub fn ingest_profile_bootstrap_report(
        &self,
        report: &ProfileBootstrapReport,
        source_ref: &str,
        occurred_at: &str,
    ) -> Result<MemoryIngestResult> {
        let source_id = stable_id("memory-source-profile-bootstrap", source_ref);
        self.ensure_source(NewMemorySource {
            id: source_id.clone(),
            source_type: MemorySourceType::ProfileBootstrap,
            source_ref: source_ref.to_string(),
            trust_level: MemoryTrustLevel::SystemObserved,
            created_at: occurred_at.to_string(),
        })?;

        let mut raw_event_ids = Vec::new();
        let mut node_ids = Vec::new();

        for evidence in &report.tech_stack_evidence {
            let (raw_id, node_id) = self.ingest_profile_evidence(
                &source_id,
                source_ref,
                "tech_stack",
                evidence,
                occurred_at,
            )?;
            raw_event_ids.push(raw_id);
            node_ids.push(node_id);
        }
        for evidence in &report.keyword_evidence {
            let (raw_id, node_id) = self.ingest_profile_evidence(
                &source_id,
                source_ref,
                "keyword",
                evidence,
                occurred_at,
            )?;
            raw_event_ids.push(raw_id);
            node_ids.push(node_id);
        }
        for theme in &report.recent_task_themes {
            let (raw_id, node_id) =
                self.ingest_profile_theme(&source_id, source_ref, theme, occurred_at)?;
            raw_event_ids.push(raw_id);
            node_ids.push(node_id);
        }

        Ok(MemoryIngestResult {
            source_id,
            raw_event_ids,
            node_ids,
        })
    }

    pub fn ingest_github_interaction(
        &self,
        event: &GitHubInteractionMemoryEvent,
    ) -> Result<MemoryIngestResult> {
        let source_id = stable_id("memory-source-github", &event.id);
        self.ensure_source(NewMemorySource {
            id: source_id.clone(),
            source_type: MemorySourceType::GithubInteraction,
            source_ref: event.id.clone(),
            trust_level: MemoryTrustLevel::ExternalGithub,
            created_at: event.occurred_at.clone(),
        })?;

        let subject_ref = event
            .issue_number
            .map(|number| format!("{}#{}", event.repo_full_name, number))
            .unwrap_or_else(|| event.repo_full_name.clone());
        let raw_id = stable_id("memory-raw-github", &event.id);
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.clone(),
            event_type: MemoryRawEventType::MaintainerReply,
            role: MemoryRole::Github,
            trust_level: MemoryTrustLevel::ExternalGithub,
            subject_type: if event.issue_number.is_some() {
                MemorySubjectType::Issue
            } else {
                MemorySubjectType::Repo
            },
            subject_ref,
            payload_json: json!({
                "interactionId": event.id,
                "repoFullName": event.repo_full_name,
                "issueNumber": event.issue_number,
                "maintainer": event.maintainer,
                "interactionType": event.interaction_type,
                "payload": event.payload_json,
            }),
            confidence: 1.0,
            occurred_at: event.occurred_at.clone(),
            created_at: event.occurred_at.clone(),
        })?;

        let mut node_ids = vec![self.ensure_raw_event_node(
            "memory-node-github",
            &raw_id,
            &event.occurred_at,
            json!({"summary": format!("github {}", event.interaction_type)}),
        )?];
        if let Some(maintainer) = event.maintainer.as_deref() {
            node_ids.push(self.ensure_entity_node(
                &raw_id,
                "maintainer",
                maintainer,
                &event.occurred_at,
            )?);
        }

        Ok(MemoryIngestResult {
            source_id,
            raw_event_ids: vec![raw_id],
            node_ids,
        })
    }

    pub fn ingest_manual_event(&self, event: &ManualMemoryEvent) -> Result<MemoryIngestResult> {
        let source_id = stable_id("memory-source-manual", &event.id);
        self.ensure_source(NewMemorySource {
            id: source_id.clone(),
            source_type: MemorySourceType::Manual,
            source_ref: event.id.clone(),
            trust_level: event.trust_level,
            created_at: event.occurred_at.clone(),
        })?;

        let raw_id = stable_id("memory-raw-manual", &event.id);
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.clone(),
            event_type: event.event_type,
            role: event.role,
            trust_level: event.trust_level,
            subject_type: event.subject_type,
            subject_ref: event.subject_ref.clone(),
            payload_json: event.payload_json.clone(),
            confidence: 1.0,
            occurred_at: event.occurred_at.clone(),
            created_at: event.occurred_at.clone(),
        })?;
        let node_id = self.ensure_raw_event_node(
            "memory-node-manual",
            &raw_id,
            &event.occurred_at,
            json!({"summary": "manual memory event"}),
        )?;
        Ok(MemoryIngestResult {
            source_id,
            raw_event_ids: vec![raw_id],
            node_ids: vec![node_id],
        })
    }

    fn ingest_profile_evidence(
        &self,
        source_id: &str,
        source_ref: &str,
        evidence_kind: &str,
        evidence: &EvidenceOutput,
        occurred_at: &str,
    ) -> Result<(String, String)> {
        let evidence_ref = format!("{source_ref}:{evidence_kind}:{}", evidence.term);
        let raw_id = stable_id("memory-raw-profile-bootstrap", &evidence_ref);
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.to_string(),
            event_type: MemoryRawEventType::Manual,
            role: MemoryRole::System,
            trust_level: MemoryTrustLevel::SystemObserved,
            subject_type: MemorySubjectType::Profile,
            subject_ref: evidence_kind.to_string(),
            payload_json: json!({
                "kind": evidence_kind,
                "term": evidence.term,
                "weight": evidence.weight,
                "count": evidence.count,
                "sources": evidence.sources,
                "projectRefs": evidence.project_refs,
                "manifestRefs": evidence.manifest_refs,
                "reason": evidence.reason,
            }),
            confidence: profile_confidence(evidence.weight),
            occurred_at: occurred_at.to_string(),
            created_at: occurred_at.to_string(),
        })?;
        let node_id = self.ensure_raw_event_node(
            "memory-node-profile-bootstrap",
            &raw_id,
            occurred_at,
            json!({"summary": format!("{evidence_kind}: {}", evidence.term)}),
        )?;
        Ok((raw_id, node_id))
    }

    fn ingest_profile_theme(
        &self,
        source_id: &str,
        source_ref: &str,
        theme: &RecentTaskTheme,
        occurred_at: &str,
    ) -> Result<(String, String)> {
        let evidence_ref = format!("{source_ref}:recent_task_theme:{}", theme.theme);
        let raw_id = stable_id("memory-raw-profile-bootstrap", &evidence_ref);
        self.ensure_raw_event(NewMemoryRawEvent {
            id: raw_id.clone(),
            source_id: source_id.to_string(),
            event_type: MemoryRawEventType::Manual,
            role: MemoryRole::System,
            trust_level: MemoryTrustLevel::SystemObserved,
            subject_type: MemorySubjectType::Profile,
            subject_ref: "recent_task_theme".to_string(),
            payload_json: json!({
                "kind": "recent_task_theme",
                "theme": theme.theme,
                "count": theme.count,
                "sources": theme.sources,
                "lastSeenAt": theme.last_seen_at,
            }),
            confidence: 0.7,
            occurred_at: occurred_at.to_string(),
            created_at: occurred_at.to_string(),
        })?;
        let node_id = self.ensure_raw_event_node(
            "memory-node-profile-bootstrap",
            &raw_id,
            occurred_at,
            json!({"summary": format!("recent_task_theme: {}", theme.theme)}),
        )?;
        Ok((raw_id, node_id))
    }

    fn ensure_source(&self, source: NewMemorySource) -> Result<String> {
        if self.store.get_source(&source.id)?.is_none() {
            self.store.insert_source(&source)?;
        }
        Ok(source.id)
    }

    fn ensure_raw_event(&self, event: NewMemoryRawEvent) -> Result<String> {
        if self.store.get_raw_event(&event.id)?.is_none() {
            self.store.insert_raw_event(&event)?;
        }
        Ok(event.id)
    }

    fn ensure_raw_event_node(
        &self,
        prefix: &str,
        raw_event_id: &str,
        created_at: &str,
        metadata_json: Value,
    ) -> Result<String> {
        let node_id = stable_id(prefix, raw_event_id);
        if self.store.get_node(&node_id)?.is_none() {
            self.store.insert_node(&NewMemoryNode {
                id: node_id.clone(),
                node_type: MemoryNodeType::RawEvent,
                raw_event_id: Some(raw_event_id.to_string()),
                entity_type: None,
                entity_value: None,
                normalized_value: None,
                text_ref: Some(format!("memory_raw_events:{raw_event_id}")),
                metadata_json,
                created_at: created_at.to_string(),
            })?;
        }
        Ok(node_id)
    }

    fn ensure_entity_node(
        &self,
        raw_event_id: &str,
        entity_type: &str,
        entity_value: &str,
        created_at: &str,
    ) -> Result<String> {
        let normalized = normalize_entity_value(entity_value);
        let node_id = stable_id(
            "memory-node-entity",
            &format!("{raw_event_id}:{entity_type}:{normalized}"),
        );
        if self.store.get_node(&node_id)?.is_none() {
            self.store.insert_node(&NewMemoryNode {
                id: node_id.clone(),
                node_type: MemoryNodeType::Entity,
                raw_event_id: Some(raw_event_id.to_string()),
                entity_type: Some(entity_type.to_string()),
                entity_value: Some(entity_value.to_string()),
                normalized_value: Some(normalized),
                text_ref: Some(format!("memory_raw_events:{raw_event_id}")),
                metadata_json: json!({}),
                created_at: created_at.to_string(),
            })?;
        }
        Ok(node_id)
    }
}

fn recommendation_event_type(event_type: RecommendationEventType) -> MemoryRawEventType {
    match event_type {
        RecommendationEventType::Dismissed => MemoryRawEventType::Dismiss,
        RecommendationEventType::Done
        | RecommendationEventType::Prepared
        | RecommendationEventType::Restored => MemoryRawEventType::Approve,
        RecommendationEventType::Shown | RecommendationEventType::Read => {
            MemoryRawEventType::Manual
        }
    }
}

fn recommendation_role(event: &RecommendationEvent) -> MemoryRole {
    match event.event_type {
        RecommendationEventType::Done
        | RecommendationEventType::Dismissed
        | RecommendationEventType::Restored => MemoryRole::User,
        RecommendationEventType::Read
            if matches!(event.source, RecommendationEventSource::CliHandoff) =>
        {
            MemoryRole::User
        }
        RecommendationEventType::Read => MemoryRole::System,
        RecommendationEventType::Shown | RecommendationEventType::Prepared => MemoryRole::System,
    }
}

fn recommendation_trust(event: &RecommendationEvent) -> MemoryTrustLevel {
    match event.event_type {
        RecommendationEventType::Done
        | RecommendationEventType::Dismissed
        | RecommendationEventType::Restored => MemoryTrustLevel::UserExplicit,
        RecommendationEventType::Read
            if matches!(event.source, RecommendationEventSource::CliHandoff) =>
        {
            MemoryTrustLevel::UserExplicit
        }
        RecommendationEventType::Shown
        | RecommendationEventType::Read
        | RecommendationEventType::Prepared => MemoryTrustLevel::SystemObserved,
    }
}

fn recommendation_confidence(event: &RecommendationEvent) -> f64 {
    match recommendation_trust(event) {
        MemoryTrustLevel::UserExplicit => 1.0,
        MemoryTrustLevel::SystemObserved => 0.8,
        MemoryTrustLevel::ExternalGithub
        | MemoryTrustLevel::AgentObserved
        | MemoryTrustLevel::LlmInferred => 0.6,
    }
}

fn profile_confidence(weight: i32) -> f64 {
    (0.5 + (weight.max(0) as f64 / 100.0)).min(0.95)
}

fn normalize_entity_value(value: &str) -> String {
    value.trim().to_lowercase()
}

fn stable_id(prefix: &str, value: &str) -> String {
    format!("{prefix}-{:016x}", stable_hash(value.as_bytes()))
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::stable_id;

    #[test]
    fn stable_ids_do_not_depend_on_process_hasher_state() {
        assert_eq!(
            stable_id("memory-raw", "owner/repo#1"),
            "memory-raw-6926c414355d3a7d"
        );
    }
}
