use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::github::IssueRef;
use crate::handoff::Handoff;
use crate::inbox;
use crate::paths::IssueFinderPaths;

use super::model::{
    AgentArtifact, IssueTask, IssueTaskPackage, IssueTaskStatus, NewArtifact, NewIssueTask,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PackageImportResult {
    pub issue_task: IssueTask,
    pub handoff_artifact: AgentArtifact,
    pub package_artifact: AgentArtifact,
    pub package: IssueTaskPackage,
}

pub fn import_handoff_from_inbox(
    store: &DispatchStore,
    inbox_id: &str,
) -> Result<PackageImportResult> {
    let item = inbox::find_item(&store.paths(), inbox_id)?;
    import_handoff_item(store, &item)
}

pub fn ensure_issue_task_for_issue(store: &DispatchStore, issue: &str) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = issue_key(&issue_ref);
    match store.find_issue_task_by_key(&issue_key)? {
        Some(issue_task) => Ok(issue_task),
        None => Ok(import_ready_handoff_for_issue(store, &issue_ref, &issue_key)?.issue_task),
    }
}

pub fn ensure_packaged_issue_task_for_issue(
    store: &DispatchStore,
    issue: &str,
) -> Result<IssueTask> {
    let issue_ref = IssueRef::parse(issue)?;
    let issue_key = issue_key(&issue_ref);
    match store.find_issue_task_by_key(&issue_key)? {
        Some(issue_task) if issue_task.current_package_artifact_id.is_some() => Ok(issue_task),
        _ => Ok(import_ready_handoff_for_issue(store, &issue_ref, &issue_key)?.issue_task),
    }
}

fn import_ready_handoff_for_issue(
    store: &DispatchStore,
    issue_ref: &IssueRef,
    issue_key: &str,
) -> Result<PackageImportResult> {
    let repo_full_name = issue_ref.repo_full_name();
    let mut candidates = inbox::load_index(&store.paths())?
        .items
        .into_iter()
        .filter(|item| {
            item.repo_full_name == repo_full_name
                && item.issue_number == issue_ref.number
                && item.status == inbox::InboxStatus::Ready
                && !item.handoff_json_path.trim().is_empty()
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.score.cmp(&left.score))
    });
    let item = candidates.into_iter().next().with_context(|| {
        format!("issue task {issue_key} has no task package artifact and no ready inbox handoff was found")
    })?;
    import_handoff_item(store, &item)
}

fn import_handoff_item(
    store: &DispatchStore,
    item: &inbox::InboxItem,
) -> Result<PackageImportResult> {
    let raw = std::fs::read(&item.handoff_json_path)
        .with_context(|| format!("unable to read {}", item.handoff_json_path))?;
    let handoff = serde_json::from_slice::<Handoff>(&raw)
        .with_context(|| format!("unable to parse {}", item.handoff_json_path))?;
    if handoff.issue.repo_full_name != item.repo_full_name
        || handoff.issue.number != item.issue_number
    {
        anyhow::bail!(
            "handoff {} describes {}#{}, but inbox item {} points to {}#{}",
            item.handoff_json_path,
            handoff.issue.repo_full_name,
            handoff.issue.number,
            item.id,
            item.repo_full_name,
            item.issue_number
        );
    }

    let issue_task = store.upsert_issue_task(NewIssueTask {
        repo_full_name: handoff.issue.repo_full_name.clone(),
        issue_number: handoff.issue.number,
        title: handoff.issue.title.clone(),
        url: handoff.issue.url.clone(),
        status: IssueTaskStatus::UserApproved,
        priority: Some(item.score.into()),
        category: Some(handoff.value_assessment.recommendation_category.to_string()),
    })?;
    if let Some(existing) = existing_import_result(store, &issue_task, &item.id)? {
        return Ok(existing);
    }

    let handoff_artifact = store.write_artifact(
        NewArtifact {
            issue_task_id: Some(issue_task.id.clone()),
            run_id: None,
            kind: "handoff_json".to_string(),
            content_type: "application/json".to_string(),
            metadata_json: json!({
                "inboxId": item.id,
                "sourcePath": item.handoff_json_path
            }),
        },
        raw,
    )?;
    let profile_snapshot = user_profile_snapshot(&store.paths());
    let profile_snapshot_artifact =
        store.write_profile_snapshot_artifact(&issue_task.id, &profile_snapshot)?;
    let package = IssueTaskPackage::from_handoff(
        &handoff,
        &handoff_artifact.id,
        json!({
            "artifactId": profile_snapshot_artifact.id,
            "snapshot": profile_snapshot
        }),
    );
    let package_artifact = store.write_task_package_artifact(&issue_task.id, &package)?;

    Ok(PackageImportResult {
        issue_task: store.get_issue_task(&issue_task.id)?,
        handoff_artifact,
        package_artifact,
        package,
    })
}

fn existing_import_result(
    store: &DispatchStore,
    issue_task: &IssueTask,
    inbox_id: &str,
) -> Result<Option<PackageImportResult>> {
    let Some(package_artifact_id) = issue_task.current_package_artifact_id.as_deref() else {
        return Ok(None);
    };
    let artifacts = store.list_artifacts_for_issue_task(&issue_task.id)?;
    let Some(handoff_artifact) = artifacts.into_iter().rev().find(|artifact| {
        artifact.kind == "handoff_json"
            && artifact
                .metadata_json
                .get("inboxId")
                .and_then(Value::as_str)
                == Some(inbox_id)
    }) else {
        return Ok(None);
    };
    let package_artifact = store.get_artifact(package_artifact_id)?;
    let package = serde_json::from_slice::<IssueTaskPackage>(
        &store.read_artifact_bytes(&package_artifact.id)?,
    )
    .context("existing IssueTaskPackage artifact is not valid JSON")?;

    Ok(Some(PackageImportResult {
        issue_task: store.get_issue_task(&issue_task.id)?,
        handoff_artifact,
        package_artifact,
        package,
    }))
}

fn issue_key(issue_ref: &IssueRef) -> String {
    format!("{}#{}", issue_ref.repo_full_name(), issue_ref.number)
}

fn user_profile_snapshot(paths: &IssueFinderPaths) -> serde_json::Value {
    match Config::load_or_default(paths) {
        Ok(config) => json!({
            "source": "config_profile",
            "profile": {
                "techStack": config.profile.tech_stack,
                "keywords": config.profile.keywords
            }
        }),
        Err(error) => json!({
            "source": "config_profile",
            "profile": null,
            "loadError": error.to_string()
        }),
    }
}
