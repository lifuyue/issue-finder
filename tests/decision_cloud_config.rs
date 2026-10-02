use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use issue_finder::config::Config;
use serde_json::Value;
use tempfile::TempDir;

const SECRET: &str = "SYNTHETIC_PRIVATE_CONFIG_SECRET";

fn configure(home: &Path, overrides: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_issue-finder"));
    command
        .env_clear()
        .env("HOME", home)
        .env("ISSUE_FINDER_HOME", home);
    command.args([
        "decision-configure",
        "--provider",
        "cloudflare-clef-flash",
        "--concurrency",
        "8",
        "--candidate-budget",
        "24",
        "--timeout-seconds",
        "45",
    ]);
    command.args(overrides).output().unwrap()
}

fn assert_json(output: &Output, success: bool) -> Value {
    assert_eq!(output.status.success(), success, "{output:?}");
    for stream in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains(SECRET));
    }
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result.is_object());
    assert_eq!(result["success"], success);
    result
}

fn config_value(home: &Path) -> toml::Value {
    toml::from_str(&fs::read_to_string(home.join("config.toml")).unwrap()).unwrap()
}

#[test]
fn creates_complete_defaults_with_explicit_settings_and_private_permissions() {
    let directory = TempDir::new().unwrap();
    let home = directory.path().join("new-state");
    let result = assert_json(&configure(&home, &[]), true);
    assert_eq!(result["provider"], "cloudflare_clef_flash");
    assert_eq!(result["concurrency"], 8);
    assert_eq!(result["candidate_budget"], 24);
    assert_eq!(result["timeout_seconds"], 45);
    let config: Config = config_value(&home).try_into().unwrap();
    assert_eq!(config.profile, Config::default().profile);
    assert_eq!(config.github.token, "");
    assert_eq!(config.llm.api_key, "");
    config.decision.validate().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(home.join("config.toml"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn preserves_all_unrelated_values_migrates_legacy_and_is_idempotent() {
    let directory = TempDir::new().unwrap();
    let home = directory.path();
    let mut original = toml::Value::try_from(Config::default()).unwrap();
    original["github"]["token"] = SECRET.into();
    original["profile"]["tech_stack"] = toml::Value::Array(vec!["CustomStack".into()]);
    original["llm"]["api_key"] = SECRET.into();
    original.as_table_mut().unwrap().insert(
        "future_config".into(),
        toml::Value::Array(vec![true.into(), 17.into()]),
    );
    let mut legacy = original.as_table_mut().unwrap().remove("decision").unwrap();
    legacy["task_preferences"] = "Only documentation and Rust fixes".into();
    legacy["cloudflare_clef_flash"]["account_id"] = "existing-account".into();
    legacy["cloudflare_clef_flash"]["endpoint"] = "http://127.0.0.1:1/never-called".into();
    original["profile"]
        .as_table_mut()
        .unwrap()
        .insert("future_profile_option".into(), SECRET.into());
    original
        .as_table_mut()
        .unwrap()
        .insert("system1".into(), legacy.clone());
    let path = home.join("config.toml");
    fs::write(&path, toml::to_string_pretty(&original).unwrap()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    }
    assert_json(&configure(home, &[]), true);
    let result = config_value(home);
    let root = original.as_table_mut().unwrap();
    root.remove("system1");
    legacy["provider"] = "cloudflare_clef_flash".into();
    legacy["concurrency"] = 8.into();
    legacy["candidate_budget"] = 24.into();
    legacy["timeout_seconds"] = 45.into();
    root.insert("decision".into(), legacy);
    assert_eq!(result, original);
    let loaded: Config = result.try_into().unwrap();
    assert_eq!(
        loaded.decision.task_preferences,
        "Only documentation and Rust fixes"
    );
    loaded.decision.validate().unwrap();
    let first = fs::read(&path).unwrap();
    assert_json(&configure(home, &[]), true);
    assert_eq!(fs::read(&path).unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
    assert!(!home.join("config.toml.tmp").exists());
}

#[test]
fn rejects_invalid_or_ambiguous_config_without_modification_or_secret_output() {
    let directory = TempDir::new().unwrap();
    let home = directory.path();
    let path = home.join("config.toml");
    let defaults = toml::to_string_pretty(&Config::default()).unwrap();
    let cases = [
        format!("[github]\ntoken = \"{SECRET}"),
        defaults.replace("top_n = 5", &format!("top_n = \"{SECRET}\"")),
        format!("{defaults}\n[system1]\nprovider = \"codex\"\n"),
        defaults.replace(
            "task_preferences = \"\"",
            &format!("task_preferences = \"\"\nunsupported_setting = \"{SECRET}\""),
        ),
        defaults.replace(
            "task_preferences = \"\"",
            &format!("task_preferences = \"{}\"", SECRET.repeat(200)),
        ),
    ];
    for raw in cases {
        fs::write(&path, &raw).unwrap();
        assert_json(&configure(home, &[]), false);
        assert_eq!(fs::read_to_string(&path).unwrap(), raw);
        assert!(!home.join("config.toml.tmp").exists());
    }
    // Semantic CLI values must fail validation before creating state.
    let fresh = home.join("fresh");
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .env_clear()
        .env("HOME", home)
        .env("ISSUE_FINDER_HOME", &fresh)
        .args([
            "decision-configure",
            "--provider",
            "cloudflare-clef-flash",
            "--concurrency",
            "0",
            "--candidate-budget",
            "24",
            "--timeout-seconds",
            "45",
        ])
        .output()
        .unwrap();
    assert_json(&output, false);
    assert!(!fresh.exists());
}

#[cfg(unix)]
#[test]
fn rejects_config_and_staging_symlinks_without_changing_targets() {
    use std::os::unix::fs::symlink;
    let directory = TempDir::new().unwrap();
    let home = directory.path();
    let target = home.join("private-target");
    fs::write(&target, SECRET).unwrap();
    symlink(&target, home.join("config.toml")).unwrap();
    assert_json(&configure(home, &[]), false);
    assert_eq!(fs::read_to_string(&target).unwrap(), SECRET);
    fs::remove_file(home.join("config.toml")).unwrap();
    symlink(&target, home.join("config.toml.tmp")).unwrap();
    assert_json(&configure(home, &[]), false);
    assert_eq!(fs::read_to_string(&target).unwrap(), SECRET);
    assert!(!home.join("config.toml").exists());
}
