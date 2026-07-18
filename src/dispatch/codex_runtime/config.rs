use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

pub fn codex_config_override_args() -> Vec<String> {
    let mut args = Vec::new();
    let provider_id = std::env::var("ISSUE_FINDER_CODEX_MODEL_PROVIDER")
        .ok()
        .filter(|value| {
            !value.is_empty()
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                })
        })
        .unwrap_or_else(|| "cliproxy".to_string());
    let mappings = [
        ("model".to_string(), "ISSUE_FINDER_CODEX_MODEL"),
        (
            "model_provider".to_string(),
            "ISSUE_FINDER_CODEX_MODEL_PROVIDER",
        ),
        (
            "model_reasoning_effort".to_string(),
            "ISSUE_FINDER_CODEX_REASONING_EFFORT",
        ),
        (
            format!("model_providers.{provider_id}.name"),
            "ISSUE_FINDER_CODEX_PROVIDER_NAME",
        ),
        (
            format!("model_providers.{provider_id}.base_url"),
            "ISSUE_FINDER_CODEX_BASE_URL",
        ),
        (
            format!("model_providers.{provider_id}.env_key"),
            "ISSUE_FINDER_CODEX_API_KEY_ENV",
        ),
        (
            format!("model_providers.{provider_id}.wire_api"),
            "ISSUE_FINDER_CODEX_WIRE_API",
        ),
    ];
    for (key, environment_name) in mappings {
        if let Ok(value) = std::env::var(environment_name) {
            if !value.is_empty() {
                args.push("-c".to_string());
                args.push(format!("{key}={}", serde_json::to_string(&value).unwrap()));
            }
        }
    }
    args
}

pub fn discover_codex_binary() -> Result<String> {
    if let Ok(explicit) = std::env::var("ISSUE_FINDER_CODEX_BIN") {
        let explicit = explicit.trim();
        if explicit.is_empty() {
            anyhow::bail!("ISSUE_FINDER_CODEX_BIN is empty");
        }
        validate_codex_binary(explicit)
            .with_context(|| format!("ISSUE_FINDER_CODEX_BIN {explicit} is not usable"))?;
        return Ok(explicit.to_string());
    }
    if let Some(path) = find_command_path("codex") {
        if validate_codex_binary(&path).is_ok() {
            return Ok(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let bundled = "/Applications/ChatGPT.app/Contents/Resources/codex";
        if validate_codex_binary(bundled).is_ok() {
            return Ok(bundled.to_string());
        }
    }
    anyhow::bail!("no usable Codex binary found; set ISSUE_FINDER_CODEX_BIN")
}

fn validate_codex_binary(command: &str) -> Result<()> {
    if !Path::new(command).is_file() {
        anyhow::bail!("binary does not exist");
    }
    for args in [
        ["--version"].as_slice(),
        ["app-server", "--help"].as_slice(),
    ] {
        let output = Command::new(command).args(args).output()?;
        if !output.status.success() {
            anyhow::bail!("{} failed", args.join(" "));
        }
    }
    Ok(())
}

fn find_command_path(command: &str) -> Option<String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() || command.contains(std::path::MAIN_SEPARATOR) {
        return command_path
            .is_file()
            .then(|| command_path.to_string_lossy().to_string());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(|directory| directory.join(command))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().to_string())
}
