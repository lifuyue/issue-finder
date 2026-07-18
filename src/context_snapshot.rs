use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::handoff::Handoff;
use crate::paths::atomic_write;

pub const CONTEXT_SNAPSHOT_FILE: &str = "context-snapshot.json";
const SNAPSHOT_KIND: &str = "issue_finder_context_snapshot";
const SNAPSHOT_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshot {
    pub kind: String,
    pub version: u8,
    pub id: String,
    pub handoff_id: String,
    pub created_at: String,
    pub workspace_path: String,
    pub files: Vec<ContextSnapshotFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextSnapshotFile {
    pub id: String,
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredContextSnapshot {
    pub snapshot: ContextSnapshot,
    pub files: Vec<StoredContextSnapshotFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredContextSnapshotFile {
    pub id: String,
    pub relative_path: String,
    pub sha256: String,
    pub media_type: String,
    pub artifact_id: String,
}

impl ContextSnapshot {
    pub fn build(root: &Path, handoff: &Handoff) -> Result<Self> {
        Self::build_with_metadata(
            root,
            &handoff.id,
            &handoff.created_at,
            &handoff.workspace.path,
        )
    }

    fn build_with_metadata(
        root: &Path,
        handoff_id: &str,
        created_at: &str,
        workspace_path: &str,
    ) -> Result<Self> {
        let mut files = Vec::new();
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(root)?;
            validate_relative_path(relative)?;
            let relative_path = slash_path(relative);
            if relative_path == CONTEXT_SNAPSHOT_FILE {
                continue;
            }
            let bytes = std::fs::read(entry.path())?;
            files.push(ContextSnapshotFile {
                id: snapshot_file_id(&relative_path),
                relative_path: relative_path.clone(),
                sha256: hex_sha256(&bytes),
                size_bytes: bytes.len() as u64,
                media_type: media_type(&relative_path).to_string(),
            });
        }
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        if files.is_empty() {
            anyhow::bail!("context snapshot cannot be empty");
        }
        for required in ["handoff.json", "codex.md", "context/entry.md"] {
            if !files.iter().any(|file| file.relative_path == required) {
                anyhow::bail!("context snapshot is missing required file {required}");
            }
        }
        let id = snapshot_id(handoff_id, &files)?;
        Ok(Self {
            kind: SNAPSHOT_KIND.to_string(),
            version: SNAPSHOT_VERSION,
            id,
            handoff_id: handoff_id.to_string(),
            created_at: created_at.to_string(),
            workspace_path: workspace_path.to_string(),
            files,
        })
    }

    pub fn write(root: &Path, handoff: &Handoff) -> Result<(Self, PathBuf)> {
        let snapshot = Self::build(root, handoff)?;
        let path = root.join(CONTEXT_SNAPSHOT_FILE);
        atomic_write(&path, serde_json::to_vec_pretty(&snapshot)?)?;
        Ok((snapshot, path))
    }

    pub fn load_and_verify(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("unable to read context snapshot {}", path.display()))?;
        let snapshot: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid context snapshot {}", path.display()))?;
        if snapshot.kind != SNAPSHOT_KIND || snapshot.version != SNAPSHOT_VERSION {
            anyhow::bail!("unsupported context snapshot kind or version");
        }
        let root = path
            .parent()
            .context("context snapshot path has no parent directory")?;
        if snapshot.files.is_empty() {
            anyhow::bail!("context snapshot cannot be empty");
        }
        for file in &snapshot.files {
            let relative = Path::new(&file.relative_path);
            validate_relative_path(relative)?;
            let bytes = std::fs::read(root.join(relative)).with_context(|| {
                format!("context snapshot file {} is missing", file.relative_path)
            })?;
            if bytes.len() as u64 != file.size_bytes || hex_sha256(&bytes) != file.sha256 {
                anyhow::bail!(
                    "context snapshot file {} failed digest verification",
                    file.relative_path
                );
            }
        }
        if snapshot.id != snapshot_id(&snapshot.handoff_id, &snapshot.files)? {
            anyhow::bail!("context snapshot id does not match its manifest");
        }
        Ok(snapshot)
    }
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        anyhow::bail!("context snapshot paths must be non-empty and relative");
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        anyhow::bail!("context snapshot path escapes its root: {}", path.display());
    }
    Ok(())
}

fn snapshot_id(handoff_id: &str, files: &[ContextSnapshotFile]) -> Result<String> {
    let manifest = serde_json::to_vec(&(handoff_id, files))?;
    Ok(format!("ctx-{}", hex_sha256(&manifest)))
}

fn snapshot_file_id(relative_path: &str) -> String {
    relative_path
        .trim_end_matches(".md")
        .trim_end_matches(".json")
        .trim_end_matches(".jsonl")
        .replace(['/', '.'], "_")
}

fn media_type(path: &str) -> &'static str {
    if path.ends_with(".json") || path.ends_with(".jsonl") {
        "application/json"
    } else if path.ends_with(".md") {
        "text/markdown"
    } else {
        "application/octet-stream"
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_detects_modified_context_file() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("context")).unwrap();
        std::fs::write(temp.path().join("handoff.json"), b"{}").unwrap();
        std::fs::write(temp.path().join("codex.md"), b"entry").unwrap();
        std::fs::write(temp.path().join("context/entry.md"), b"context").unwrap();
        let snapshot = ContextSnapshot::build_with_metadata(
            temp.path(),
            "handoff-1",
            "2026-07-18T00:00:00Z",
            "/tmp/workspace",
        )
        .unwrap();
        let path = temp.path().join(CONTEXT_SNAPSHOT_FILE);
        atomic_write(&path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
        ContextSnapshot::load_and_verify(&path).unwrap();
        std::fs::write(temp.path().join("codex.md"), b"changed").unwrap();
        assert!(ContextSnapshot::load_and_verify(&path).is_err());
    }
}
