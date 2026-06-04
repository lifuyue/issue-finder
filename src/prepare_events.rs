use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use serde_json::{Map, Value};

use crate::github::GitHubIssue;

#[derive(Debug, Clone)]
pub struct PrepareEventLog {
    path: PathBuf,
}

impl PrepareEventLog {
    pub fn create(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, [])?;
        Ok(Self { path })
    }

    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event_type: &str, fields: &[(&str, Value)]) -> Result<()> {
        let mut event = Map::new();
        event.insert("type".to_string(), Value::String(event_type.to_string()));
        event.insert(
            "timestamp".to_string(),
            Value::String(Utc::now().to_rfc3339()),
        );
        for (key, value) in fields {
            event.insert((*key).to_string(), value.clone());
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", serde_json::to_string(&event)?)?;
        Ok(())
    }

    pub fn append_prepare_started(&self, issue: &GitHubIssue) -> Result<()> {
        self.append(
            "prepare_started",
            &[(
                "issue",
                Value::String(format!("{}#{}", issue.repo_full_name, issue.number)),
            )],
        )
    }
}
