//! Explicit provider selection. Errors never silently change models or accounts.
use std::time::Duration;

use crate::config::{DecisionConfig, DecisionProvider};

use super::aliyun::AliyunProvider;
use super::clef::ClefProvider;
use super::codex::CodexProvider;
use super::contract::{DecisionFuture, DecisionRequest, Provider, ProviderError};

pub enum ConfiguredProvider {
    Aliyun(Box<AliyunProvider>),
    Clef(Box<ClefProvider>),
    Codex(Box<CodexProvider>),
}

impl ConfiguredProvider {
    pub fn new(config: &DecisionConfig) -> Result<Self, ProviderError> {
        config
            .validate()
            .map_err(|error| ProviderError::new("invalid_configuration", error.to_string()))?;
        let timeout = Duration::from_secs(config.timeout_seconds);
        match config.provider {
            DecisionProvider::AliyunDecision => {
                let endpoint = configured_value(
                    "ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT",
                    &config.aliyun_decision.endpoint,
                )?;
                if endpoint.is_empty() {
                    return Err(ProviderError::new("configuration_required", "Set ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT or decision.aliyun_decision.endpoint to the full workspace-specific /compatible-mode/v1/systemone URL"));
                }
                let key = credential("DASHSCOPE_API_KEY")?;
                Ok(Self::Aliyun(Box::new(AliyunProvider::new(
                    endpoint, key, timeout,
                )?)))
            }
            DecisionProvider::CloudflareClefFlash => {
                let selection = cloudflare_selection()?;
                let selected_account = selection
                    .as_ref()
                    .map(|(_, account_env)| configured_value(account_env, ""))
                    .transpose()?;
                if selected_account
                    .as_deref()
                    .is_some_and(|account| !valid_account(account))
                {
                    return Err(ProviderError::new("configuration_required", "The selected Cloudflare account environment variable must contain a nonempty valid account ID"));
                }
                let endpoint = if config.cloudflare_clef_flash.endpoint.trim().is_empty() {
                    let account = match &selected_account {
                        Some(account) => account.clone(),
                        None => configured_value(
                            "CLOUDFLARE_ACCOUNT_ID",
                            &config.cloudflare_clef_flash.account_id,
                        )?,
                    };
                    if !valid_account(&account) {
                        return Err(ProviderError::new("configuration_required", "Set CLOUDFLARE_ACCOUNT_ID or decision.cloudflare_clef_flash.account_id to a valid account ID"));
                    }
                    format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/@cf/cloudflare/clef-flash")
                } else {
                    config.cloudflare_clef_flash.endpoint.trim().to_owned()
                };
                if let Some(account) = selected_account {
                    let expected_path =
                        format!("/client/v4/accounts/{account}/ai/run/@cf/cloudflare/clef-flash");
                    if !url::Url::parse(&endpoint).is_ok_and(|url| url.path() == expected_path) {
                        return Err(ProviderError::new("invalid_configuration", "Configured Cloudflare endpoint does not match the explicitly selected account"));
                    }
                }
                let key = credential(
                    selection
                        .as_ref()
                        .map_or("CLOUDFLARE_API_TOKEN", |(token_env, _)| token_env),
                )?;
                Ok(Self::Clef(Box::new(ClefProvider::new(
                    endpoint, key, timeout,
                )?)))
            }
            DecisionProvider::Codex => {
                let binary =
                    (!config.codex_binary.trim().is_empty()).then(|| config.codex_binary.clone());
                Ok(Self::Codex(Box::new(CodexProvider::new(binary, timeout)?)))
            }
        }
    }

    pub async fn close(&self) {
        if let Self::Codex(provider) = self {
            provider.close().await;
        }
    }

    fn inner(&self) -> &dyn Provider {
        match self {
            Self::Aliyun(provider) => provider.as_ref(),
            Self::Clef(provider) => provider.as_ref(),
            Self::Codex(provider) => provider.as_ref(),
        }
    }
}

impl Provider for ConfiguredProvider {
    fn fingerprint(&self) -> String {
        self.inner().fingerprint()
    }

    fn decide<'a>(&'a self, request: &'a DecisionRequest) -> DecisionFuture<'a> {
        self.inner().decide(request)
    }
}

fn configured_value(key: &str, configured: &str) -> Result<String, ProviderError> {
    match std::env::var(key) {
        Ok(value) => Ok(value.trim().to_owned()),
        Err(std::env::VarError::NotPresent) => Ok(configured.trim().to_owned()),
        Err(_) => Err(ProviderError::new(
            "invalid_configuration",
            format!("{key} must be UTF-8"),
        )),
    }
}

fn valid_account(account: &str) -> bool {
    !account.is_empty()
        && account
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn cloudflare_selection() -> Result<Option<(String, String)>, ProviderError> {
    let token = environment_selector("ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV")?;
    let account = environment_selector("ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV")?;
    match (token, account) {
        (None, None) => Ok(None),
        (Some(token), Some(account)) => Ok(Some((token, account))),
        _ => Err(ProviderError::new("invalid_configuration", "Set both ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV and ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV when selecting Cloudflare credentials")),
    }
}

fn environment_selector(selector: &str) -> Result<Option<String>, ProviderError> {
    match std::env::var(selector) {
        Ok(name)
            if !name.is_empty()
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')) =>
        {
            Ok(Some(name))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        _ => Err(ProviderError::new(
                "invalid_configuration",
                format!("{selector} must name an environment variable using ASCII letters, digits, underscores or hyphens"),
            )),
    }
}

fn credential(key: &str) -> Result<String, ProviderError> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ProviderError::new("authentication_setup", format!("{key} is missing or empty; configure it through the environment credential mechanism")))
}
