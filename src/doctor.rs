use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::github::GitHubClient;
use crate::paths::PatchbayPaths;
use crate::workspace::git_available;

const DOCTOR_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

pub async fn run_doctor(paths: &PatchbayPaths, config: Option<&Config>) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    checks.push(DoctorCheck {
        name: "git".to_string(),
        ok: git_available(),
        message: if git_available() {
            "Git is available".to_string()
        } else {
            "Git was not found in PATH".to_string()
        },
    });

    checks.push(DoctorCheck {
        name: "config".to_string(),
        ok: paths.config.exists(),
        message: if paths.config.exists() {
            format!("Config exists at {}", paths.config.display())
        } else {
            "Config is missing; run `patchbay init`".to_string()
        },
    });

    for (name, path) in [
        ("home", &paths.home),
        ("cache", &paths.cache_dir),
        ("workspaces", &paths.workspaces_dir),
        ("inbox", &paths.inbox_dir),
        ("reports", &paths.reports_dir),
    ] {
        let writable = directory_writable(path);
        checks.push(DoctorCheck {
            name: format!("path:{name}"),
            ok: path.exists() && writable,
            message: if path.exists() && writable {
                format!("{} is writable", path.display())
            } else if path.exists() {
                format!("{} exists but is not writable", path.display())
            } else {
                format!("{} is missing", path.display())
            },
        });
    }

    if let Some(config) = config {
        checks.push(DoctorCheck {
            name: "github_token".to_string(),
            ok: !config.github.token.trim().is_empty(),
            message: if config.github.token.trim().is_empty() {
                "GitHub token is missing".to_string()
            } else {
                "GitHub token is configured".to_string()
            },
        });

        if !config.github.token.trim().is_empty() {
            let github = GitHubClient::new(config);
            match github {
                Ok(client) => match client.validate_token().await {
                    Ok(login) => checks.push(DoctorCheck {
                        name: "github_auth".to_string(),
                        ok: true,
                        message: format!("Authenticated as {login}"),
                    }),
                    Err(error) => checks.push(DoctorCheck {
                        name: "github_auth".to_string(),
                        ok: false,
                        message: error.to_string(),
                    }),
                },
                Err(error) => checks.push(DoctorCheck {
                    name: "github_auth".to_string(),
                    ok: false,
                    message: error.to_string(),
                }),
            }
        }

        if config.llm.enabled {
            let api_key = config.resolved_llm_api_key();
            if api_key.trim().is_empty() {
                checks.push(DoctorCheck {
                    name: "llm".to_string(),
                    ok: false,
                    message: "LLM is enabled but API key is missing".to_string(),
                });
            } else {
                match check_llm_reachable(config, &api_key).await {
                    Ok(()) => checks.push(DoctorCheck {
                        name: "llm".to_string(),
                        ok: true,
                        message: format!("LLM endpoint is reachable at {}", config.llm.base_url),
                    }),
                    Err(error) => checks.push(DoctorCheck {
                        name: "llm".to_string(),
                        ok: false,
                        message: error.to_string(),
                    }),
                }
            }
        }
    }

    checks.push(DoctorCheck {
        name: "platform".to_string(),
        ok: true,
        message: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    });

    checks
}

pub fn render_doctor(checks: &[DoctorCheck]) -> String {
    checks
        .iter()
        .map(|check| {
            format!(
                "[{}] {} - {}",
                if check.ok { "ok" } else { "fail" },
                check.name,
                check.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn ensure_paths(paths: &PatchbayPaths) -> Result<()> {
    paths.ensure_layout()
}

fn directory_writable(path: &std::path::Path) -> bool {
    if !path.exists() {
        return false;
    }

    let probe = path.join(".patchbay-doctor-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

async fn check_llm_reachable(config: &Config, api_key: &str) -> Result<()> {
    let url = format!("{}/models", config.llm.base_url.trim_end_matches('/'));
    let response = reqwest::Client::builder()
        .user_agent("patchbay-cli")
        .timeout(DOCTOR_HTTP_TIMEOUT)
        .build()?
        .get(url)
        .bearer_auth(api_key.trim())
        .send()
        .await?;

    if response.status().is_success() {
        Ok(())
    } else {
        anyhow::bail!("LLM endpoint returned {}", response.status())
    }
}
