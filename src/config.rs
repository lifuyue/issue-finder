use std::fs;
use std::io::{self, Write};

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
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubConfig {
    pub token: String,
    pub username: String,
    #[serde(default)]
    pub api_base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGitHubToken {
    pub token: String,
    pub source: GitHubTokenSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHubTokenSource {
    EnvGithubToken,
    Config,
    Missing,
}

impl GitHubTokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvGithubToken => "env:GITHUB_TOKEN",
            Self::Config => "config",
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
    #[serde(default = "default_llm_wire_api")]
    pub wire_api: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentConfig {
    #[serde(default = "default_agent_context_max_items")]
    pub context_max_items: usize,
    #[serde(default = "default_agent_context_max_chars")]
    pub context_max_chars: usize,
    #[serde(default = "default_agent_context_max_item_chars")]
    pub context_max_item_chars: usize,
    #[serde(default = "default_agent_context_recent_items_after_compaction")]
    pub context_recent_items_after_compaction: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            context_max_items: default_agent_context_max_items(),
            context_max_chars: default_agent_context_max_chars(),
            context_max_item_chars: default_agent_context_max_item_chars(),
            context_recent_items_after_compaction:
                default_agent_context_recent_items_after_compaction(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            github: GitHubConfig {
                token: std::env::var("GITHUB_TOKEN").unwrap_or_default(),
                username: String::new(),
                api_base_url: String::new(),
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
                wire_api: default_llm_wire_api(),
            },
            agent: AgentConfig::default(),
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
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if !token.trim().is_empty() {
                return ResolvedGitHubToken {
                    token,
                    source: GitHubTokenSource::EnvGithubToken,
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

    pub fn resolved_github_api_base_url(&self) -> String {
        if !self.github.api_base_url.trim().is_empty() {
            return self.github.api_base_url.trim().to_string();
        }

        std::env::var("ISSUE_FINDER_GITHUB_API_BASE")
            .unwrap_or_else(|_| "https://api.github.com".to_string())
    }
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
    config.github.token = prompt("GitHub token", &config.github.token)?;
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

fn default_llm_wire_api() -> String {
    "chat_completions".to_string()
}

fn default_agent_context_max_items() -> usize {
    12
}

fn default_agent_context_max_chars() -> usize {
    24_000
}

fn default_agent_context_max_item_chars() -> usize {
    4_000
}

fn default_agent_context_recent_items_after_compaction() -> usize {
    8
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn default_config_matches_spec_shape() {
        let config = Config::default();
        assert_eq!(config.daily.top_n, 5);
        assert!(!config.llm.enabled);
        assert_eq!(config.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(config.llm.wire_api, "chat_completions");
    }

    #[test]
    fn llm_config_accepts_explicit_wire_api_and_defaults_legacy_configs() {
        let legacy = toml::from_str::<Config>(
            r#"
[github]
token = ""
username = ""

[profile]
tech_stack = ["Rust"]
keywords = ["cli"]

[daily]
top_n = 5

[llm]
enabled = true
base_url = "https://api.example.test/v1"
api_key = ""
api_key_env = "TEST_KEY"
model = "test-model"
"#,
        )
        .unwrap();
        assert_eq!(legacy.llm.wire_api, "chat_completions");

        let responses = toml::from_str::<Config>(
            r#"
[github]
token = ""
username = ""

[profile]
tech_stack = ["Rust"]
keywords = ["cli"]

[daily]
top_n = 5

[llm]
enabled = true
base_url = "https://api.example.test/v1"
api_key = ""
api_key_env = "TEST_KEY"
model = "test-model"
wire_api = "responses"
"#,
        )
        .unwrap();
        assert_eq!(responses.llm.wire_api, "responses");
    }
}
