use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::paths::{atomic_write, sanitize_repo_name, IssueFinderPaths};

use super::model::{
    AgentArtifact, AgentCapability, AgentCapabilityName, AgentEvent, AgentProfile,
    AgentSessionLink, AgentSessionStatus, ApprovalRequest, ApprovalStatus, ApprovalType,
    CapabilityStatus, DispatchFailureClass, DispatchOutcomeKind, DispatchRun, DispatchRunOutcome,
    DispatchRunStatus, DispatchTaskClass, DispatchValidationOutcome, GitHubInteraction,
    GitHubInteractionStatus, GitHubInteractionType, IssueTask, IssueTaskPackage, IssueTaskStatus,
    MemoryEvent, MemoryEventType, NewAgentCapability, NewAgentEvent, NewAgentProfile,
    NewAgentSessionLink, NewApprovalRequest, NewArtifact, NewDispatchRun, NewDispatchRunOutcome,
    NewGitHubInteraction, NewIssueTask, NewMemoryEvent,
};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

pub struct DispatchStore {
    paths: IssueFinderPaths,
    conn: Connection,
}

impl DispatchStore {
    pub fn open(paths: IssueFinderPaths) -> Result<Self> {
        let db_path = paths.dispatch_db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("unable to create {}", parent.display()))?;
        }
        let conn = Connection::open(&db_path)
            .with_context(|| format!("unable to open {}", db_path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        initialize_schema(&conn)?;
        Ok(Self { paths, conn })
    }

    pub fn db_path(&self) -> PathBuf {
        self.paths.dispatch_db_path()
    }

    pub fn paths(&self) -> IssueFinderPaths {
        self.paths.clone()
    }

    pub fn create_agent_profile(&self, input: NewAgentProfile) -> Result<AgentProfile> {
        let id = input.id.unwrap_or_else(|| next_id("agent"));
        self.conn.execute(
            "INSERT INTO agent_profiles (id, kind, display_name, adapter, config_json, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                input.kind,
                input.display_name,
                input.adapter,
                json_text(&input.config_json)?,
                bool_int(input.enabled)
            ],
        )?;
        self.get_agent_profile(&id)
            .with_context(|| format!("agent profile {id} was not persisted"))
    }

    pub fn ensure_agent_profile(&self, input: NewAgentProfile) -> Result<AgentProfile> {
        let Some(id) = input.id.as_deref() else {
            return self.create_agent_profile(input);
        };

        if let Some(profile) = self.find_agent_profile(id)? {
            return Ok(profile);
        }

        self.create_agent_profile(input)
    }

    pub fn find_agent_profile(&self, id: &str) -> Result<Option<AgentProfile>> {
        let profile = self
            .conn
            .query_row(
                "SELECT id, kind, display_name, adapter, config_json, enabled
                 FROM agent_profiles
                 WHERE id = ?1",
                params![id],
                agent_profile_from_row,
            )
            .optional()?;
        Ok(profile)
    }

    pub fn get_agent_profile(&self, id: &str) -> Result<AgentProfile> {
        self.conn
            .query_row(
                "SELECT id, kind, display_name, adapter, config_json, enabled
                 FROM agent_profiles
                 WHERE id = ?1",
                params![id],
                agent_profile_from_row,
            )
            .with_context(|| format!("agent profile {id} not found"))
    }

    pub fn list_agent_profiles(&self) -> Result<Vec<AgentProfile>> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, display_name, adapter, config_json, enabled
             FROM agent_profiles
             ORDER BY id",
        )?;
        let rows = statement.query_map([], agent_profile_from_row)?;
        collect_rows(rows)
    }

    pub fn upsert_agent_capability(&self, input: NewAgentCapability) -> Result<AgentCapability> {
        self.conn.execute(
            "INSERT INTO agent_capabilities (agent_id, capability, status, details_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent_id, capability)
             DO UPDATE SET status = excluded.status, details_json = excluded.details_json",
            params![
                input.agent_id,
                input.capability.as_str(),
                input.status.as_str(),
                json_text(&input.details_json)?
            ],
        )?;
        self.get_agent_capability(&input.agent_id, input.capability)
    }

    pub fn get_agent_capability(
        &self,
        agent_id: &str,
        capability: AgentCapabilityName,
    ) -> Result<AgentCapability> {
        self.conn
            .query_row(
                "SELECT agent_id, capability, status, details_json
                 FROM agent_capabilities
                 WHERE agent_id = ?1 AND capability = ?2",
                params![agent_id, capability.as_str()],
                agent_capability_from_row,
            )
            .with_context(|| {
                format!(
                    "agent capability {} for {agent_id} not found",
                    capability.as_str()
                )
            })
    }

    pub fn list_agent_capabilities(&self, agent_id: &str) -> Result<Vec<AgentCapability>> {
        let mut statement = self.conn.prepare(
            "SELECT agent_id, capability, status, details_json
             FROM agent_capabilities
             WHERE agent_id = ?1
             ORDER BY capability",
        )?;
        let rows = statement.query_map(params![agent_id], agent_capability_from_row)?;
        collect_rows(rows)
    }

    pub fn upsert_issue_task(&self, input: NewIssueTask) -> Result<IssueTask> {
        let now = now();
        let id = next_id("issue-task");
        let issue_key = issue_key(&input.repo_full_name, input.issue_number);
        self.conn.execute(
            "INSERT INTO issue_tasks (
                id, issue_key, repo_full_name, issue_number, title, url, status,
                priority, category, created_at, updated_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
             ON CONFLICT(issue_key)
             DO UPDATE SET
                title = excluded.title,
                url = excluded.url,
                status = excluded.status,
                priority = excluded.priority,
                category = excluded.category,
                updated_at = excluded.updated_at",
            params![
                id,
                issue_key,
                input.repo_full_name,
                issue_number_i64(input.issue_number)?,
                input.title,
                input.url,
                input.status.as_str(),
                input.priority,
                input.category,
                now
            ],
        )?;
        self.get_issue_task_by_key(&issue_key)
    }

    pub fn get_issue_task(&self, id: &str) -> Result<IssueTask> {
        self.conn
            .query_row(
                "SELECT id, issue_key, repo_full_name, issue_number, title, url, status,
                        priority, category, created_at, updated_at,
                        current_package_artifact_id, profile_snapshot_artifact_id
                 FROM issue_tasks
                 WHERE id = ?1",
                params![id],
                issue_task_from_row,
            )
            .with_context(|| format!("issue task {id} not found"))
    }

    pub fn get_issue_task_by_key(&self, issue_key: &str) -> Result<IssueTask> {
        self.conn
            .query_row(
                "SELECT id, issue_key, repo_full_name, issue_number, title, url, status,
                        priority, category, created_at, updated_at,
                        current_package_artifact_id, profile_snapshot_artifact_id
                 FROM issue_tasks
                 WHERE issue_key = ?1",
                params![issue_key],
                issue_task_from_row,
            )
            .with_context(|| format!("issue task {issue_key} not found"))
    }

    pub fn find_issue_task_by_key(&self, issue_key: &str) -> Result<Option<IssueTask>> {
        let issue_task = self
            .conn
            .query_row(
                "SELECT id, issue_key, repo_full_name, issue_number, title, url, status,
                        priority, category, created_at, updated_at,
                        current_package_artifact_id, profile_snapshot_artifact_id
                 FROM issue_tasks
                 WHERE issue_key = ?1",
                params![issue_key],
                issue_task_from_row,
            )
            .optional()?;
        Ok(issue_task)
    }

    pub fn set_issue_task_package_artifact(
        &self,
        issue_task_id: &str,
        artifact_id: &str,
    ) -> Result<IssueTask> {
        self.conn.execute(
            "UPDATE issue_tasks
             SET current_package_artifact_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![issue_task_id, artifact_id, now()],
        )?;
        self.get_issue_task(issue_task_id)
    }

    pub fn set_issue_task_profile_snapshot_artifact(
        &self,
        issue_task_id: &str,
        artifact_id: &str,
    ) -> Result<IssueTask> {
        self.conn.execute(
            "UPDATE issue_tasks
             SET profile_snapshot_artifact_id = ?2, updated_at = ?3
             WHERE id = ?1",
            params![issue_task_id, artifact_id, now()],
        )?;
        self.get_issue_task(issue_task_id)
    }

    pub fn update_issue_task_status(
        &self,
        issue_task_id: &str,
        status: IssueTaskStatus,
    ) -> Result<IssueTask> {
        self.conn.execute(
            "UPDATE issue_tasks
             SET status = ?2, updated_at = ?3
             WHERE id = ?1",
            params![issue_task_id, status.as_str(), now()],
        )?;
        self.get_issue_task(issue_task_id)
    }

    pub fn create_dispatch_run(&self, input: NewDispatchRun) -> Result<DispatchRun> {
        let id = next_id("dispatch-run");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO dispatch_runs (
                id, issue_task_id, agent_id, status, requested_by, approval_state,
                created_at, selected_session_link_id
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                input.issue_task_id,
                input.agent_id,
                input.status.as_str(),
                input.requested_by,
                input.approval_state.as_str(),
                created_at,
                input.selected_session_link_id
            ],
        )?;
        self.get_dispatch_run(&id)
            .with_context(|| format!("dispatch run {id} was not persisted"))
    }

    pub fn get_dispatch_run(&self, id: &str) -> Result<DispatchRun> {
        self.conn
            .query_row(
                "SELECT id, issue_task_id, agent_id, status, requested_by, approval_state,
                        created_at, started_at, completed_at, selected_session_link_id,
                        result_artifact_id, failure_reason
                 FROM dispatch_runs
                 WHERE id = ?1",
                params![id],
                dispatch_run_from_row,
            )
            .with_context(|| format!("dispatch run {id} not found"))
    }

    pub fn set_dispatch_run_session(
        &self,
        run_id: &str,
        session_link_id: &str,
    ) -> Result<DispatchRun> {
        self.conn.execute(
            "UPDATE dispatch_runs
             SET selected_session_link_id = ?2
             WHERE id = ?1",
            params![run_id, session_link_id],
        )?;
        self.get_dispatch_run(run_id)
    }

    pub fn set_dispatch_run_result_artifact(
        &self,
        run_id: &str,
        artifact_id: &str,
    ) -> Result<DispatchRun> {
        self.conn.execute(
            "UPDATE dispatch_runs
             SET result_artifact_id = ?2
             WHERE id = ?1",
            params![run_id, artifact_id],
        )?;
        self.get_dispatch_run(run_id)
    }

    pub fn update_dispatch_run_approval_state(
        &self,
        run_id: &str,
        approval_state: ApprovalStatus,
    ) -> Result<DispatchRun> {
        self.conn.execute(
            "UPDATE dispatch_runs
             SET approval_state = ?2
             WHERE id = ?1",
            params![run_id, approval_state.as_str()],
        )?;
        self.get_dispatch_run(run_id)
    }

    pub fn update_dispatch_run_status(
        &self,
        run_id: &str,
        status: DispatchRunStatus,
        failure_reason: Option<String>,
    ) -> Result<DispatchRun> {
        let now = now();
        let started_at = if dispatch_started(status) {
            Some(now.as_str())
        } else {
            None
        };
        let completed_at = if dispatch_terminal(status) {
            Some(now.as_str())
        } else {
            None
        };
        self.conn.execute(
            "UPDATE dispatch_runs
             SET status = ?2,
                 started_at = COALESCE(started_at, ?3),
                 completed_at = COALESCE(?4, completed_at),
                 failure_reason = ?5
             WHERE id = ?1",
            params![
                run_id,
                status.as_str(),
                started_at,
                completed_at,
                failure_reason
            ],
        )?;
        self.get_dispatch_run(run_id)
    }

    pub fn record_dispatch_run_outcome(
        &self,
        input: NewDispatchRunOutcome,
    ) -> Result<DispatchRunOutcome> {
        if let Some(existing) = self.find_dispatch_run_outcome_by_run(&input.run_id)? {
            if dispatch_outcome_matches(&existing, &input) {
                return Ok(existing);
            }
            anyhow::bail!(
                "dispatch run {} already has outcome {} with idempotency key {}",
                input.run_id,
                existing.id,
                existing.idempotency_key
            );
        }
        if let Some(existing) =
            self.find_dispatch_run_outcome_by_idempotency_key(&input.idempotency_key)?
        {
            if dispatch_outcome_matches(&existing, &input) {
                return Ok(existing);
            }
            anyhow::bail!(
                "dispatch outcome idempotency key {} already belongs to run {}",
                input.idempotency_key,
                existing.run_id
            );
        }

        let id = next_id("dispatch-outcome");
        let recorded_at = now();
        self.conn.execute(
            "INSERT INTO dispatch_run_outcomes (
                id, run_id, idempotency_key, outcome_kind, failure_class, failure_detail,
                task_class, validation_outcome, result_artifact_id, metadata_json, recorded_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                input.run_id,
                input.idempotency_key,
                input.outcome_kind.as_str(),
                input.failure_class.map(DispatchFailureClass::as_str),
                input.failure_detail,
                input.task_class.map(DispatchTaskClass::as_str),
                input
                    .validation_outcome
                    .map(DispatchValidationOutcome::as_str),
                input.result_artifact_id,
                json_text(&input.metadata_json)?,
                recorded_at
            ],
        )?;
        self.get_dispatch_run_outcome(&id)
            .with_context(|| format!("dispatch run outcome {id} was not persisted"))
    }

    pub fn get_dispatch_run_outcome(&self, id: &str) -> Result<DispatchRunOutcome> {
        self.conn
            .query_row(
                "SELECT id, run_id, idempotency_key, outcome_kind, failure_class,
                        failure_detail, task_class, validation_outcome, result_artifact_id,
                        metadata_json, recorded_at
                 FROM dispatch_run_outcomes
                 WHERE id = ?1",
                params![id],
                dispatch_run_outcome_from_row,
            )
            .with_context(|| format!("dispatch run outcome {id} not found"))
    }

    pub fn find_dispatch_run_outcome_by_run(
        &self,
        run_id: &str,
    ) -> Result<Option<DispatchRunOutcome>> {
        self.conn
            .query_row(
                "SELECT id, run_id, idempotency_key, outcome_kind, failure_class,
                        failure_detail, task_class, validation_outcome, result_artifact_id,
                        metadata_json, recorded_at
                 FROM dispatch_run_outcomes
                 WHERE run_id = ?1",
                params![run_id],
                dispatch_run_outcome_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn find_dispatch_run_outcome_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<DispatchRunOutcome>> {
        self.conn
            .query_row(
                "SELECT id, run_id, idempotency_key, outcome_kind, failure_class,
                        failure_detail, task_class, validation_outcome, result_artifact_id,
                        metadata_json, recorded_at
                 FROM dispatch_run_outcomes
                 WHERE idempotency_key = ?1",
                params![idempotency_key],
                dispatch_run_outcome_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn create_session_link(&self, input: NewAgentSessionLink) -> Result<AgentSessionLink> {
        let id = next_id("session-link");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_session_links (
                id, agent_id, native_session_id, issue_task_id, display_name, goal,
                status, metadata_json, created_at, last_seen_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                id,
                input.agent_id,
                input.native_session_id,
                input.issue_task_id,
                input.display_name,
                input.goal,
                input.status.as_str(),
                json_text(&input.metadata_json)?,
                created_at
            ],
        )?;
        self.get_session_link(&id)
            .with_context(|| format!("session link {id} was not persisted"))
    }

    pub fn get_session_link(&self, id: &str) -> Result<AgentSessionLink> {
        self.conn
            .query_row(
                "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                        status, metadata_json, created_at, last_seen_at, archived_at
                 FROM agent_session_links
                 WHERE id = ?1",
                params![id],
                agent_session_link_from_row,
            )
            .with_context(|| format!("session link {id} not found"))
    }

    pub fn update_session_link_status(
        &self,
        session_link_id: &str,
        status: AgentSessionStatus,
    ) -> Result<AgentSessionLink> {
        let now = now();
        let archived_at = if status == AgentSessionStatus::Archived {
            Some(now.as_str())
        } else {
            None
        };
        self.conn.execute(
            "UPDATE agent_session_links
             SET status = ?2,
                 last_seen_at = ?3,
                 archived_at = COALESCE(?4, archived_at)
             WHERE id = ?1",
            params![session_link_id, status.as_str(), now, archived_at],
        )?;
        self.get_session_link(session_link_id)
    }

    pub fn rename_session_link(
        &self,
        session_link_id: &str,
        display_name: &str,
    ) -> Result<AgentSessionLink> {
        self.conn.execute(
            "UPDATE agent_session_links
             SET display_name = ?2,
                 last_seen_at = ?3
             WHERE id = ?1",
            params![session_link_id, display_name, now()],
        )?;
        self.get_session_link(session_link_id)
    }

    pub fn list_session_links_for_issue_task(
        &self,
        issue_task_id: &str,
    ) -> Result<Vec<AgentSessionLink>> {
        let mut statement = self.conn.prepare(
            "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                    status, metadata_json, created_at, last_seen_at, archived_at
             FROM agent_session_links
             WHERE issue_task_id = ?1
             ORDER BY last_seen_at DESC, id",
        )?;
        let rows = statement.query_map(params![issue_task_id], agent_session_link_from_row)?;
        collect_rows(rows)
    }

    pub fn list_session_links(&self, agent_id: Option<&str>) -> Result<Vec<AgentSessionLink>> {
        if let Some(agent_id) = agent_id {
            let mut statement = self.conn.prepare(
                "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                        status, metadata_json, created_at, last_seen_at, archived_at
                 FROM agent_session_links
                 WHERE agent_id = ?1
                 ORDER BY last_seen_at DESC, id",
            )?;
            let rows = statement.query_map(params![agent_id], agent_session_link_from_row)?;
            return collect_rows(rows);
        }

        let mut statement = self.conn.prepare(
            "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                    status, metadata_json, created_at, last_seen_at, archived_at
             FROM agent_session_links
             ORDER BY last_seen_at DESC, id",
        )?;
        let rows = statement.query_map([], agent_session_link_from_row)?;
        collect_rows(rows)
    }

    pub fn find_session_link_by_native_id(
        &self,
        agent_id: &str,
        native_session_id: &str,
    ) -> Result<AgentSessionLink> {
        self.conn
            .query_row(
                "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                        status, metadata_json, created_at, last_seen_at, archived_at
                 FROM agent_session_links
                 WHERE agent_id = ?1 AND native_session_id = ?2",
                params![agent_id, native_session_id],
                agent_session_link_from_row,
            )
            .with_context(|| {
                format!("session link {native_session_id} for agent {agent_id} not found")
            })
    }

    pub fn find_session_link_by_native_id_opt(
        &self,
        agent_id: &str,
        native_session_id: &str,
    ) -> Result<Option<AgentSessionLink>> {
        self.conn
            .query_row(
                "SELECT id, agent_id, native_session_id, issue_task_id, display_name, goal,
                        status, metadata_json, created_at, last_seen_at, archived_at
                 FROM agent_session_links
                 WHERE agent_id = ?1 AND native_session_id = ?2",
                params![agent_id, native_session_id],
                agent_session_link_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn append_agent_event(&self, input: NewAgentEvent) -> Result<AgentEvent> {
        let id = next_id("agent-event");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO agent_events (
                id, run_id, session_link_id, event_type, native_event_id, payload_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                input.run_id,
                input.session_link_id,
                input.event_type,
                input.native_event_id,
                json_text(&input.payload_json)?,
                created_at
            ],
        )?;
        self.get_agent_event(&id)
            .with_context(|| format!("agent event {id} was not persisted"))
    }

    pub fn get_agent_event(&self, id: &str) -> Result<AgentEvent> {
        self.conn
            .query_row(
                "SELECT id, run_id, session_link_id, event_type, native_event_id,
                        payload_json, created_at
                 FROM agent_events
                 WHERE id = ?1",
                params![id],
                agent_event_from_row,
            )
            .with_context(|| format!("agent event {id} not found"))
    }

    pub fn list_agent_events_for_run(&self, run_id: &str) -> Result<Vec<AgentEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT id, run_id, session_link_id, event_type, native_event_id,
                    payload_json, created_at
             FROM agent_events
             WHERE run_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![run_id], agent_event_from_row)?;
        collect_rows(rows)
    }

    pub fn write_artifact(
        &self,
        input: NewArtifact,
        contents: impl AsRef<[u8]>,
    ) -> Result<AgentArtifact> {
        let bytes = contents.as_ref();
        let id = next_id("artifact");
        let created_at = now();
        let sha256 = sha256_hex(bytes);
        let path = artifact_path(&self.paths, &id, &input, &input.content_type);
        atomic_write(&path, bytes)?;
        let path = path.to_string_lossy().to_string();
        let insert_result = self.conn.execute(
            "INSERT INTO agent_artifacts (
                id, issue_task_id, run_id, kind, path, content_type, sha256,
                created_at, metadata_json
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                input.issue_task_id,
                input.run_id,
                input.kind,
                path,
                input.content_type,
                sha256,
                created_at,
                json_text(&input.metadata_json)?
            ],
        );
        if let Err(error) = insert_result {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
        self.get_artifact(&id)
            .with_context(|| format!("artifact {id} was not persisted"))
    }

    pub fn write_task_package_artifact(
        &self,
        issue_task_id: &str,
        package: &IssueTaskPackage,
    ) -> Result<AgentArtifact> {
        let artifact = self.write_artifact(
            NewArtifact {
                issue_task_id: Some(issue_task_id.to_string()),
                run_id: None,
                kind: "issue_task_package".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: serde_json::json!({
                    "packageKind": package.kind.as_str(),
                    "packageVersion": package.version
                }),
            },
            serde_json::to_vec_pretty(package)?,
        )?;
        self.set_issue_task_package_artifact(issue_task_id, &artifact.id)?;
        Ok(artifact)
    }

    pub fn write_profile_snapshot_artifact(
        &self,
        issue_task_id: &str,
        snapshot: &serde_json::Value,
    ) -> Result<AgentArtifact> {
        let artifact = self.write_artifact(
            NewArtifact {
                issue_task_id: Some(issue_task_id.to_string()),
                run_id: None,
                kind: "user_profile_snapshot".to_string(),
                content_type: "application/json".to_string(),
                metadata_json: serde_json::json!({
                    "source": "config_profile"
                }),
            },
            serde_json::to_vec_pretty(snapshot)?,
        )?;
        self.set_issue_task_profile_snapshot_artifact(issue_task_id, &artifact.id)?;
        Ok(artifact)
    }

    pub fn get_artifact(&self, id: &str) -> Result<AgentArtifact> {
        self.conn
            .query_row(
                "SELECT id, issue_task_id, run_id, kind, path, content_type, sha256,
                        created_at, metadata_json
                 FROM agent_artifacts
                 WHERE id = ?1",
                params![id],
                agent_artifact_from_row,
            )
            .with_context(|| format!("artifact {id} not found"))
    }

    pub fn read_artifact_bytes(&self, id: &str) -> Result<Vec<u8>> {
        let artifact = self.get_artifact(id)?;
        let bytes = std::fs::read(&artifact.path)
            .with_context(|| format!("unable to read artifact {}", artifact.path))?;
        let actual = sha256_hex(&bytes);
        if actual != artifact.sha256 {
            anyhow::bail!(
                "artifact {} failed sha256 verification: expected {}, got {}",
                artifact.id,
                artifact.sha256,
                actual
            );
        }
        Ok(bytes)
    }

    pub fn list_artifacts_for_issue_task(&self, issue_task_id: &str) -> Result<Vec<AgentArtifact>> {
        let mut statement = self.conn.prepare(
            "SELECT id, issue_task_id, run_id, kind, path, content_type, sha256,
                    created_at, metadata_json
             FROM agent_artifacts
             WHERE issue_task_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![issue_task_id], agent_artifact_from_row)?;
        collect_rows(rows)
    }

    pub fn list_artifacts_for_run(&self, run_id: &str) -> Result<Vec<AgentArtifact>> {
        let mut statement = self.conn.prepare(
            "SELECT id, issue_task_id, run_id, kind, path, content_type, sha256,
                    created_at, metadata_json
             FROM agent_artifacts
             WHERE run_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![run_id], agent_artifact_from_row)?;
        collect_rows(rows)
    }

    pub fn create_github_interaction(
        &self,
        input: NewGitHubInteraction,
    ) -> Result<GitHubInteraction> {
        let id = next_id("github-interaction");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO github_interactions (
                id, issue_task_id, interaction_type, body_artifact_id, status, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                input.issue_task_id,
                input.interaction_type.as_str(),
                input.body_artifact_id,
                input.status.as_str(),
                created_at
            ],
        )?;
        self.get_github_interaction(&id)
            .with_context(|| format!("github interaction {id} was not persisted"))
    }

    pub fn get_github_interaction(&self, id: &str) -> Result<GitHubInteraction> {
        self.conn
            .query_row(
                "SELECT id, issue_task_id, interaction_type, github_comment_id,
                        body_artifact_id, status, created_at, posted_at, error
                 FROM github_interactions
                 WHERE id = ?1",
                params![id],
                github_interaction_from_row,
            )
            .with_context(|| format!("github interaction {id} not found"))
    }

    pub fn mark_github_interaction_posted(
        &self,
        id: &str,
        github_comment_id: &str,
    ) -> Result<GitHubInteraction> {
        self.conn.execute(
            "UPDATE github_interactions
             SET status = ?2,
                 github_comment_id = ?3,
                 posted_at = ?4,
                 error = NULL
             WHERE id = ?1",
            params![
                id,
                GitHubInteractionStatus::Posted.as_str(),
                github_comment_id,
                now()
            ],
        )?;
        self.get_github_interaction(id)
    }

    pub fn mark_github_interaction_failed(
        &self,
        id: &str,
        error: impl Into<String>,
    ) -> Result<GitHubInteraction> {
        self.conn.execute(
            "UPDATE github_interactions
             SET status = ?2,
                 error = ?3
             WHERE id = ?1",
            params![id, GitHubInteractionStatus::Failed.as_str(), error.into()],
        )?;
        self.get_github_interaction(id)
    }

    pub fn update_github_interaction_status(
        &self,
        id: &str,
        status: GitHubInteractionStatus,
    ) -> Result<GitHubInteraction> {
        self.conn.execute(
            "UPDATE github_interactions
             SET status = ?2
             WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        self.get_github_interaction(id)
    }

    pub fn list_github_interactions_for_issue_task(
        &self,
        issue_task_id: &str,
    ) -> Result<Vec<GitHubInteraction>> {
        let mut statement = self.conn.prepare(
            "SELECT id, issue_task_id, interaction_type, github_comment_id,
                    body_artifact_id, status, created_at, posted_at, error
             FROM github_interactions
             WHERE issue_task_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![issue_task_id], github_interaction_from_row)?;
        collect_rows(rows)
    }

    pub fn create_approval_request(&self, input: NewApprovalRequest) -> Result<ApprovalRequest> {
        let id = next_id("approval");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO approval_requests (
                id, run_id, approval_type, status, prompt, details_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                input.run_id,
                input.approval_type.as_str(),
                input.status.as_str(),
                input.prompt,
                json_text(&input.details_json)?,
                created_at
            ],
        )?;
        self.get_approval_request(&id)
            .with_context(|| format!("approval request {id} was not persisted"))
    }

    pub fn get_approval_request(&self, id: &str) -> Result<ApprovalRequest> {
        self.conn
            .query_row(
                "SELECT id, run_id, approval_type, status, prompt, details_json,
                        created_at, resolved_at
                 FROM approval_requests
                 WHERE id = ?1",
                params![id],
                approval_request_from_row,
            )
            .with_context(|| format!("approval request {id} not found"))
    }

    pub fn resolve_approval_request(
        &self,
        id: &str,
        status: ApprovalStatus,
    ) -> Result<ApprovalRequest> {
        let resolved_at = if status == ApprovalStatus::Pending {
            None
        } else {
            Some(now())
        };
        self.conn.execute(
            "UPDATE approval_requests
             SET status = ?2,
                 resolved_at = ?3
             WHERE id = ?1",
            params![id, status.as_str(), resolved_at],
        )?;
        self.get_approval_request(id)
    }

    pub fn list_approval_requests_for_run(&self, run_id: &str) -> Result<Vec<ApprovalRequest>> {
        let mut statement = self.conn.prepare(
            "SELECT id, run_id, approval_type, status, prompt, details_json,
                    created_at, resolved_at
             FROM approval_requests
             WHERE run_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![run_id], approval_request_from_row)?;
        collect_rows(rows)
    }

    pub fn list_approval_requests_by_type(
        &self,
        approval_type: ApprovalType,
    ) -> Result<Vec<ApprovalRequest>> {
        let mut statement = self.conn.prepare(
            "SELECT id, run_id, approval_type, status, prompt, details_json,
                    created_at, resolved_at
             FROM approval_requests
             WHERE approval_type = ?1
             ORDER BY created_at, id",
        )?;
        let rows =
            statement.query_map(params![approval_type.as_str()], approval_request_from_row)?;
        collect_rows(rows)
    }

    pub fn append_memory_event(&self, input: NewMemoryEvent) -> Result<MemoryEvent> {
        let id = next_id("memory-event");
        let created_at = now();
        self.conn.execute(
            "INSERT INTO memory_events (
                id, issue_task_id, event_type, source, payload_json, created_at
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                input.issue_task_id,
                input.event_type.as_str(),
                input.source,
                json_text(&input.payload_json)?,
                created_at
            ],
        )?;
        self.get_memory_event(&id)
            .with_context(|| format!("memory event {id} was not persisted"))
    }

    pub fn get_memory_event(&self, id: &str) -> Result<MemoryEvent> {
        self.conn
            .query_row(
                "SELECT id, issue_task_id, event_type, source, payload_json, created_at
                 FROM memory_events
                 WHERE id = ?1",
                params![id],
                memory_event_from_row,
            )
            .with_context(|| format!("memory event {id} not found"))
    }

    pub fn list_memory_events_for_issue_task(
        &self,
        issue_task_id: &str,
    ) -> Result<Vec<MemoryEvent>> {
        let mut statement = self.conn.prepare(
            "SELECT id, issue_task_id, event_type, source, payload_json, created_at
             FROM memory_events
             WHERE issue_task_id = ?1
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map(params![issue_task_id], memory_event_from_row)?;
        collect_rows(rows)
    }
}

fn initialize_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_profiles (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            display_name TEXT NOT NULL,
            adapter TEXT NOT NULL,
            config_json TEXT NOT NULL,
            enabled INTEGER NOT NULL CHECK (enabled IN (0, 1))
        );

        CREATE TABLE IF NOT EXISTS agent_capabilities (
            agent_id TEXT NOT NULL,
            capability TEXT NOT NULL,
            status TEXT NOT NULL,
            details_json TEXT NOT NULL,
            PRIMARY KEY (agent_id, capability),
            FOREIGN KEY (agent_id) REFERENCES agent_profiles(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS issue_tasks (
            id TEXT PRIMARY KEY,
            issue_key TEXT NOT NULL UNIQUE,
            repo_full_name TEXT NOT NULL,
            issue_number INTEGER NOT NULL,
            title TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL,
            priority INTEGER,
            category TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            current_package_artifact_id TEXT,
            profile_snapshot_artifact_id TEXT
        );

        CREATE TABLE IF NOT EXISTS dispatch_runs (
            id TEXT PRIMARY KEY,
            issue_task_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            status TEXT NOT NULL,
            requested_by TEXT NOT NULL,
            approval_state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            started_at TEXT,
            completed_at TEXT,
            selected_session_link_id TEXT,
            result_artifact_id TEXT,
            failure_reason TEXT,
            FOREIGN KEY (issue_task_id) REFERENCES issue_tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (agent_id) REFERENCES agent_profiles(id) ON DELETE RESTRICT,
            FOREIGN KEY (selected_session_link_id) REFERENCES agent_session_links(id)
        );

        CREATE TABLE IF NOT EXISTS dispatch_run_outcomes (
            id TEXT PRIMARY KEY,
            run_id TEXT NOT NULL UNIQUE,
            idempotency_key TEXT NOT NULL UNIQUE,
            outcome_kind TEXT NOT NULL,
            failure_class TEXT,
            failure_detail TEXT,
            task_class TEXT,
            validation_outcome TEXT,
            result_artifact_id TEXT,
            metadata_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            FOREIGN KEY (run_id) REFERENCES dispatch_runs(id) ON DELETE CASCADE,
            FOREIGN KEY (result_artifact_id) REFERENCES agent_artifacts(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS agent_session_links (
            id TEXT PRIMARY KEY,
            agent_id TEXT NOT NULL,
            native_session_id TEXT NOT NULL,
            issue_task_id TEXT,
            display_name TEXT NOT NULL,
            goal TEXT,
            status TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_seen_at TEXT NOT NULL,
            archived_at TEXT,
            UNIQUE (agent_id, native_session_id),
            FOREIGN KEY (agent_id) REFERENCES agent_profiles(id) ON DELETE CASCADE,
            FOREIGN KEY (issue_task_id) REFERENCES issue_tasks(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS agent_events (
            id TEXT PRIMARY KEY,
            run_id TEXT,
            session_link_id TEXT,
            event_type TEXT NOT NULL,
            native_event_id TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (run_id) REFERENCES dispatch_runs(id) ON DELETE CASCADE,
            FOREIGN KEY (session_link_id) REFERENCES agent_session_links(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS agent_artifacts (
            id TEXT PRIMARY KEY,
            issue_task_id TEXT,
            run_id TEXT,
            kind TEXT NOT NULL,
            path TEXT NOT NULL,
            content_type TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            created_at TEXT NOT NULL,
            metadata_json TEXT NOT NULL,
            FOREIGN KEY (issue_task_id) REFERENCES issue_tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (run_id) REFERENCES dispatch_runs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS github_interactions (
            id TEXT PRIMARY KEY,
            issue_task_id TEXT NOT NULL,
            interaction_type TEXT NOT NULL,
            github_comment_id TEXT,
            body_artifact_id TEXT,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL,
            posted_at TEXT,
            error TEXT,
            FOREIGN KEY (issue_task_id) REFERENCES issue_tasks(id) ON DELETE CASCADE,
            FOREIGN KEY (body_artifact_id) REFERENCES agent_artifacts(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS approval_requests (
            id TEXT PRIMARY KEY,
            run_id TEXT,
            approval_type TEXT NOT NULL,
            status TEXT NOT NULL,
            prompt TEXT NOT NULL,
            details_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved_at TEXT,
            FOREIGN KEY (run_id) REFERENCES dispatch_runs(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS memory_events (
            id TEXT PRIMARY KEY,
            issue_task_id TEXT,
            event_type TEXT NOT NULL,
            source TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (issue_task_id) REFERENCES issue_tasks(id) ON DELETE SET NULL
        );

        CREATE INDEX IF NOT EXISTS idx_issue_tasks_issue_key ON issue_tasks(issue_key);
        CREATE INDEX IF NOT EXISTS idx_dispatch_runs_issue_task ON dispatch_runs(issue_task_id);
        CREATE INDEX IF NOT EXISTS idx_dispatch_run_outcomes_run ON dispatch_run_outcomes(run_id);
        CREATE INDEX IF NOT EXISTS idx_agent_session_links_issue_task ON agent_session_links(issue_task_id);
        CREATE INDEX IF NOT EXISTS idx_agent_artifacts_issue_task ON agent_artifacts(issue_task_id);
        CREATE INDEX IF NOT EXISTS idx_github_interactions_issue_task ON github_interactions(issue_task_id);
        PRAGMA user_version = 2;
        "#,
    )?;
    Ok(())
}

fn agent_profile_from_row(row: &Row<'_>) -> rusqlite::Result<AgentProfile> {
    let config: String = row.get(4)?;
    Ok(AgentProfile {
        id: row.get(0)?,
        kind: row.get(1)?,
        display_name: row.get(2)?,
        adapter: row.get(3)?,
        config_json: parse_json(&config)?,
        enabled: int_bool(row.get(5)?),
    })
}

fn agent_capability_from_row(row: &Row<'_>) -> rusqlite::Result<AgentCapability> {
    let capability: String = row.get(1)?;
    let status: String = row.get(2)?;
    let details: String = row.get(3)?;
    Ok(AgentCapability {
        agent_id: row.get(0)?,
        capability: parse_enum(&capability, AgentCapabilityName::parse_value)?,
        status: parse_enum(&status, CapabilityStatus::parse_value)?,
        details_json: parse_json(&details)?,
    })
}

fn issue_task_from_row(row: &Row<'_>) -> rusqlite::Result<IssueTask> {
    let issue_number: i64 = row.get(3)?;
    let status: String = row.get(6)?;
    Ok(IssueTask {
        id: row.get(0)?,
        issue_key: row.get(1)?,
        repo_full_name: row.get(2)?,
        issue_number: u64::try_from(issue_number).map_err(integral_value_out_of_range)?,
        title: row.get(4)?,
        url: row.get(5)?,
        status: parse_enum(&status, IssueTaskStatus::parse_value)?,
        priority: row.get(7)?,
        category: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        current_package_artifact_id: row.get(11)?,
        profile_snapshot_artifact_id: row.get(12)?,
    })
}

fn dispatch_run_from_row(row: &Row<'_>) -> rusqlite::Result<DispatchRun> {
    let status: String = row.get(3)?;
    let approval_state: String = row.get(5)?;
    Ok(DispatchRun {
        id: row.get(0)?,
        issue_task_id: row.get(1)?,
        agent_id: row.get(2)?,
        status: parse_enum(&status, DispatchRunStatus::parse_value)?,
        requested_by: row.get(4)?,
        approval_state: parse_enum(&approval_state, ApprovalStatus::parse_value)?,
        created_at: row.get(6)?,
        started_at: row.get(7)?,
        completed_at: row.get(8)?,
        selected_session_link_id: row.get(9)?,
        result_artifact_id: row.get(10)?,
        failure_reason: row.get(11)?,
    })
}

fn dispatch_run_outcome_from_row(row: &Row<'_>) -> rusqlite::Result<DispatchRunOutcome> {
    let outcome_kind: String = row.get(3)?;
    let failure_class: Option<String> = row.get(4)?;
    let task_class: Option<String> = row.get(6)?;
    let validation_outcome: Option<String> = row.get(7)?;
    let metadata: String = row.get(9)?;
    Ok(DispatchRunOutcome {
        id: row.get(0)?,
        run_id: row.get(1)?,
        idempotency_key: row.get(2)?,
        outcome_kind: parse_enum(&outcome_kind, DispatchOutcomeKind::parse_value)?,
        failure_class: failure_class
            .as_deref()
            .map(|value| parse_enum(value, DispatchFailureClass::parse_value))
            .transpose()?,
        failure_detail: row.get(5)?,
        task_class: task_class
            .as_deref()
            .map(|value| parse_enum(value, DispatchTaskClass::parse_value))
            .transpose()?,
        validation_outcome: validation_outcome
            .as_deref()
            .map(|value| parse_enum(value, DispatchValidationOutcome::parse_value))
            .transpose()?,
        result_artifact_id: row.get(8)?,
        metadata_json: parse_json(&metadata)?,
        recorded_at: row.get(10)?,
    })
}

fn agent_session_link_from_row(row: &Row<'_>) -> rusqlite::Result<AgentSessionLink> {
    let status: String = row.get(6)?;
    let metadata: String = row.get(7)?;
    Ok(AgentSessionLink {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        native_session_id: row.get(2)?,
        issue_task_id: row.get(3)?,
        display_name: row.get(4)?,
        goal: row.get(5)?,
        status: parse_enum(&status, AgentSessionStatus::parse_value)?,
        metadata_json: parse_json(&metadata)?,
        created_at: row.get(8)?,
        last_seen_at: row.get(9)?,
        archived_at: row.get(10)?,
    })
}

fn agent_event_from_row(row: &Row<'_>) -> rusqlite::Result<AgentEvent> {
    let payload: String = row.get(5)?;
    Ok(AgentEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        session_link_id: row.get(2)?,
        event_type: row.get(3)?,
        native_event_id: row.get(4)?,
        payload_json: parse_json(&payload)?,
        created_at: row.get(6)?,
    })
}

fn agent_artifact_from_row(row: &Row<'_>) -> rusqlite::Result<AgentArtifact> {
    let metadata: String = row.get(8)?;
    Ok(AgentArtifact {
        id: row.get(0)?,
        issue_task_id: row.get(1)?,
        run_id: row.get(2)?,
        kind: row.get(3)?,
        path: row.get(4)?,
        content_type: row.get(5)?,
        sha256: row.get(6)?,
        created_at: row.get(7)?,
        metadata_json: parse_json(&metadata)?,
    })
}

fn github_interaction_from_row(row: &Row<'_>) -> rusqlite::Result<GitHubInteraction> {
    let interaction_type: String = row.get(2)?;
    let status: String = row.get(5)?;
    Ok(GitHubInteraction {
        id: row.get(0)?,
        issue_task_id: row.get(1)?,
        interaction_type: parse_enum(&interaction_type, GitHubInteractionType::parse_value)?,
        github_comment_id: row.get(3)?,
        body_artifact_id: row.get(4)?,
        status: parse_enum(&status, GitHubInteractionStatus::parse_value)?,
        created_at: row.get(6)?,
        posted_at: row.get(7)?,
        error: row.get(8)?,
    })
}

fn approval_request_from_row(row: &Row<'_>) -> rusqlite::Result<ApprovalRequest> {
    let approval_type: String = row.get(2)?;
    let status: String = row.get(3)?;
    let details: String = row.get(5)?;
    Ok(ApprovalRequest {
        id: row.get(0)?,
        run_id: row.get(1)?,
        approval_type: parse_enum(&approval_type, ApprovalType::parse_value)?,
        status: parse_enum(&status, ApprovalStatus::parse_value)?,
        prompt: row.get(4)?,
        details_json: parse_json(&details)?,
        created_at: row.get(6)?,
        resolved_at: row.get(7)?,
    })
}

fn memory_event_from_row(row: &Row<'_>) -> rusqlite::Result<MemoryEvent> {
    let event_type: String = row.get(2)?;
    let payload: String = row.get(4)?;
    Ok(MemoryEvent {
        id: row.get(0)?,
        issue_task_id: row.get(1)?,
        event_type: parse_enum(&event_type, MemoryEventType::parse_value)?,
        source: row.get(3)?,
        payload_json: parse_json(&payload)?,
        created_at: row.get(5)?,
    })
}

fn collect_rows<T, F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<T>>
where
    F: FnMut(&Row<'_>) -> rusqlite::Result<T>,
{
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

fn issue_key(repo_full_name: &str, issue_number: u64) -> String {
    format!("{repo_full_name}#{issue_number}")
}

fn issue_number_i64(value: u64) -> Result<i64> {
    i64::try_from(value).context("issue number does not fit into SQLite INTEGER")
}

fn json_text(value: &Value) -> Result<String> {
    Ok(serde_json::to_string(value)?)
}

fn parse_json(value: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn parse_enum<T>(value: &str, parse: impl FnOnce(&str) -> Option<T>) -> rusqlite::Result<T> {
    parse(value).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown enum value {value}").into(),
        )
    })
}

fn int_bool(value: i64) -> bool {
    value != 0
}

fn bool_int(value: bool) -> i64 {
    if value {
        1
    } else {
        0
    }
}

fn dispatch_outcome_matches(existing: &DispatchRunOutcome, input: &NewDispatchRunOutcome) -> bool {
    existing.run_id == input.run_id
        && existing.idempotency_key == input.idempotency_key
        && existing.outcome_kind == input.outcome_kind
        && existing.failure_class == input.failure_class
        && existing.failure_detail == input.failure_detail
        && existing.task_class == input.task_class
        && existing.validation_outcome == input.validation_outcome
        && existing.result_artifact_id == input.result_artifact_id
        && existing.metadata_json == input.metadata_json
}

fn dispatch_started(status: DispatchRunStatus) -> bool {
    matches!(
        status,
        DispatchRunStatus::Starting
            | DispatchRunStatus::Running
            | DispatchRunStatus::NeedsUser
            | DispatchRunStatus::Completed
            | DispatchRunStatus::Failed
            | DispatchRunStatus::Canceled
    )
}

fn dispatch_terminal(status: DispatchRunStatus) -> bool {
    matches!(
        status,
        DispatchRunStatus::Completed | DispatchRunStatus::Failed | DispatchRunStatus::Canceled
    )
}

fn integral_value_out_of_range(error: std::num::TryFromIntError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Integer, Box::new(error))
}

fn next_id(prefix: &str) -> String {
    let timestamp = Utc::now().timestamp_millis();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{timestamp}-{counter}")
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn artifact_path(
    paths: &IssueFinderPaths,
    id: &str,
    input: &NewArtifact,
    content_type: &str,
) -> PathBuf {
    let owner = input
        .issue_task_id
        .as_deref()
        .or(input.run_id.as_deref())
        .unwrap_or("unassigned");
    paths
        .dispatch_artifacts_dir()
        .join(sanitize_repo_name(owner))
        .join(format!(
            "{}-{}.{}",
            sanitize_repo_name(id),
            sanitize_repo_name(&input.kind),
            artifact_extension(content_type)
        ))
}

fn artifact_extension(content_type: &str) -> &'static str {
    match content_type {
        "application/json" => "json",
        "text/markdown" => "md",
        value if value.starts_with("text/") => "txt",
        _ => "bin",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}
