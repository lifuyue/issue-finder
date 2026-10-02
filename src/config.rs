use std::fs;
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::errors::IssueFinderError;
use crate::paths::{atomic_write, IssueFinderPaths};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub github: GitHubConfig,
    pub profile: ProfileConfig,
    pub daily: DailyConfig,
    pub llm: LlmConfig,
    #[serde(default, alias = "system1")]
    pub decision: DecisionConfig,
}

/// Screening is independent of the legacy optional LLM reviewer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct DecisionConfig {
    pub provider: DecisionProvider,
    pub aliyun_decision: AliyunDecisionConfig,
    pub cloudflare_clef_flash: ClefFlashConfig,
    pub concurrency: usize,
    pub codex_binary: String,
    pub timeout_seconds: u64,
    pub candidate_budget: usize,
    pub task_preferences: String,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            provider: DecisionProvider::default(),
            aliyun_decision: AliyunDecisionConfig::default(),
            cloudflare_clef_flash: ClefFlashConfig::default(),
            concurrency: 4,
            codex_binary: String::new(),
            timeout_seconds: 45,
            candidate_budget: 24,
            task_preferences: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProvider {
    #[default]
    AliyunDecision,
    CloudflareClefFlash,
    Codex,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AliyunDecisionConfig {
    /// Full workspace-specific System One URL. Credentials are environment-only.
    pub endpoint: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ClefFlashConfig {
    pub account_id: String,
    /// Optional full endpoint override, primarily for local offline mocks.
    pub endpoint: String,
}

impl DecisionConfig {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.concurrency > 0,
            "decision.concurrency must be positive"
        );
        anyhow::ensure!(
            (1..=300).contains(&self.timeout_seconds),
            "decision.timeout_seconds must be between 1 and 300"
        );
        anyhow::ensure!(
            self.candidate_budget <= 100,
            "decision.candidate_budget must be between 0 and 100"
        );
        anyhow::ensure!(
            self.task_preferences.chars().count() <= 4_000,
            "decision.task_preferences must not exceed 4000 characters"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubConfig {
    pub token: String,
    pub username: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGitHubToken {
    pub token: String,
    pub source: GitHubTokenSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubTokenSource {
    EnvGhToken,
    Config,
    GitHubCli,
    Missing,
}

impl GitHubTokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvGhToken => "env:GH_TOKEN",
            Self::Config => "config",
            Self::GitHubCli => "gh",
            Self::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileConfig {
    pub tech_stack: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DailyConfig {
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    pub enabled: bool,
    pub base_url: String,
    pub api_key: String,
    pub api_key_env: String,
    pub model: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            github: GitHubConfig {
                token: String::new(),
                username: String::new(),
            },
            profile: ProfileConfig {
                tech_stack: vec!["Rust".to_string(), "TypeScript".to_string()],
                keywords: vec!["cli".to_string(), "developer-tools".to_string()],
            },
            daily: DailyConfig { top_n: 5 },
            llm: LlmConfig {
                enabled: false,
                base_url: "https://api.openai.com/v1".to_string(),
                api_key: String::new(),
                api_key_env: String::new(),
                model: "gpt-4o-mini".to_string(),
            },
            decision: DecisionConfig::default(),
        }
    }
}

impl Config {
    pub fn load(paths: &IssueFinderPaths) -> Result<Self> {
        if !paths.config.exists() {
            return Err(IssueFinderError::MissingConfig.into());
        }

        let raw = fs::read_to_string(&paths.config)
            .with_context(|| format!("unable to read {}", paths.config.display()))?;
        let config = toml::from_str(&raw)
            .with_context(|| format!("unable to parse {}", paths.config.display()))?;
        Ok(config)
    }

    pub fn load_or_default(paths: &IssueFinderPaths) -> Result<Self> {
        if paths.config.exists() {
            Self::load(paths)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, paths: &IssueFinderPaths) -> Result<()> {
        paths.ensure_layout()?;
        let raw = toml::to_string_pretty(self)?;
        atomic_write(&paths.config, raw)?;
        Ok(())
    }

    pub fn resolved_llm_api_key(&self) -> String {
        if !self.llm.api_key_env.trim().is_empty() {
            return std::env::var(self.llm.api_key_env.trim()).unwrap_or_default();
        }

        self.llm.api_key.clone()
    }

    pub fn resolved_github_token(&self) -> ResolvedGitHubToken {
        if let Ok(token) = std::env::var("GH_TOKEN") {
            if !token.trim().is_empty() {
                return ResolvedGitHubToken {
                    token,
                    source: GitHubTokenSource::EnvGhToken,
                };
            }
        }

        if !self.github.token.trim().is_empty() {
            return ResolvedGitHubToken {
                token: self.github.token.clone(),
                source: GitHubTokenSource::Config,
            };
        }

        ResolvedGitHubToken {
            token: String::new(),
            source: GitHubTokenSource::Missing,
        }
    }

    /// Session tools can reuse the current host's GitHub CLI login without
    /// copying its secret into configuration or exposing subprocess output.
    pub fn resolved_session_github_token(&self) -> ResolvedGitHubToken {
        let resolved = self.resolved_github_token();
        if resolved.source != GitHubTokenSource::Missing {
            return resolved;
        }

        github_cli_token()
            .map(|token| ResolvedGitHubToken {
                token,
                source: GitHubTokenSource::GitHubCli,
            })
            .unwrap_or(resolved)
    }
}

fn github_cli_token() -> Option<String> {
    let mut child = Command::new("gh")
        .args(["auth", "token", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1")
        // The fallback must read stored login, never the deprecated environment alias.
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }

    let mut output = String::new();
    child
        .stdout
        .take()?
        .take(16_385)
        .read_to_string(&mut output)
        .ok()?;
    let token = output.trim();
    if token.is_empty() || output.len() > 16_384 || token.chars().any(char::is_whitespace) {
        return None;
    }
    Some(token.to_string())
}

pub fn initialize_interactive(paths: &IssueFinderPaths, force: bool) -> Result<Config> {
    if paths.config.exists() && !force {
        anyhow::bail!(
            "{} already exists. Use `issue-finder init --force` to overwrite it.",
            paths.config.display()
        );
    }

    paths.ensure_layout()?;
    let mut config = Config::default();

    println!("Issue Finder config: {}", paths.config.display());
    config.github.token = prompt("GitHub token (optional; GH_TOKEN is read at runtime)", "")?;
    config.github.username = prompt("GitHub username (optional)", &config.github.username)?;
    config.profile.tech_stack = prompt_list("Tech stack", &config.profile.tech_stack)?;
    config.profile.keywords = prompt_list("Profile keywords", &config.profile.keywords)?;

    let top_n = prompt("Daily Top N", &config.daily.top_n.to_string())?;
    config.daily.top_n = top_n.parse::<usize>().unwrap_or(config.daily.top_n).max(1);

    let enable_llm = prompt("Enable optional LLM enhancement? (y/N)", "N")?;
    config.llm.enabled = matches!(enable_llm.trim().to_lowercase().as_str(), "y" | "yes");
    if config.llm.enabled {
        config.llm.base_url = prompt("LLM base URL", &config.llm.base_url)?;
        config.llm.model = prompt("LLM model", &config.llm.model)?;
        config.llm.api_key_env = prompt("LLM API key env var (optional)", &config.llm.api_key_env)?;
        if config.llm.api_key_env.trim().is_empty() {
            config.llm.api_key = prompt("LLM API key", &config.llm.api_key)?;
        }
    }

    config.save(paths)?;
    Ok(config)
}

fn prompt(label: &str, default: &str) -> Result<String> {
    print!("{label}");
    if !default.is_empty() {
        print!(" [{default}]");
    }
    print!(": ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn prompt_list(label: &str, default: &[String]) -> Result<Vec<String>> {
    let default_text = default.join(", ");
    let input = prompt(label, &default_text)?;
    Ok(input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}
