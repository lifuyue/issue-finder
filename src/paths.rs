use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct PatchbayPaths {
    pub home: PathBuf,
    pub config: PathBuf,
    pub cache_dir: PathBuf,
    pub workspaces_dir: PathBuf,
    pub inbox_dir: PathBuf,
    pub reports_dir: PathBuf,
}

impl PatchbayPaths {
    pub fn resolve() -> Result<Self> {
        let home = match env::var("PATCHBAY_HOME") {
            Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
            _ => dirs::home_dir()
                .context("unable to determine home directory")?
                .join(".patchbay"),
        };

        Ok(Self {
            config: home.join("config.toml"),
            cache_dir: home.join("cache"),
            workspaces_dir: home.join("workspaces"),
            inbox_dir: home.join("inbox"),
            reports_dir: home.join("reports"),
            home,
        })
    }

    pub fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(&self.cache_dir)?;
        fs::create_dir_all(&self.workspaces_dir)?;
        fs::create_dir_all(&self.inbox_dir)?;
        fs::create_dir_all(&self.reports_dir)?;
        Ok(())
    }

    pub fn issue_cache_path(&self) -> PathBuf {
        self.cache_dir.join("github-issues.json")
    }

    pub fn inbox_index_path(&self) -> PathBuf {
        self.inbox_dir.join("index.json")
    }

    pub fn workspace_path_for(&self, repo_full_name: &str) -> PathBuf {
        self.workspaces_dir.join(sanitize_repo_name(repo_full_name))
    }

    pub fn inbox_item_dir(&self, id: &str) -> PathBuf {
        self.inbox_dir.join(id)
    }

    pub fn report_path(&self, date: &str) -> PathBuf {
        self.reports_dir.join(format!("{date}.md"))
    }
}

pub fn sanitize_repo_name(repo_full_name: &str) -> String {
    repo_full_name.replace('/', "__")
}

pub fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_path = path.with_extension(format!(
        "{}tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .map(|value| format!("{value}."))
            .unwrap_or_default()
    ));
    fs::write(&tmp_path, contents)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sanitize_repo_name;

    #[test]
    fn sanitizes_repo_name_for_local_paths() {
        assert_eq!(sanitize_repo_name("owner/repo"), "owner__repo");
    }
}
