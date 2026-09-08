use std::fs;
use std::process::Command;

use issue_finder::config::{Config, GitHubTokenSource};
use tempfile::TempDir;

// Authentication probes run in child processes, so PATH and credentials never
// change for concurrent tests and no real host login can be consulted.
#[test]
fn auth_probe() {
    let Ok(expected_source) = std::env::var("ISSUE_FINDER_TEST_TOKEN_SOURCE") else {
        return;
    };
    let mut config = Config::default();
    config.github.token = std::env::var("ISSUE_FINDER_TEST_CONFIG_TOKEN").unwrap_or_default();
    let resolved = config.resolved_session_github_token();
    assert_eq!(resolved.source.as_str(), expected_source);
    assert_eq!(
        resolved.token,
        std::env::var("ISSUE_FINDER_TEST_EXPECTED_TOKEN").unwrap_or_default()
    );
    if expected_source == "gh" {
        assert_eq!(
            config.resolved_github_token().source,
            GitHubTokenSource::Missing
        );
    }
}

#[cfg(unix)]
fn gh_fixture(script: &str) -> TempDir {
    use std::os::unix::fs::PermissionsExt;
    let fixture = TempDir::new().unwrap();
    let path = fixture.path().join("gh");
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    fixture
}

fn probe(path: &std::path::Path, source: &str, token: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "auth_probe", "--nocapture"])
        .env("PATH", path)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("ISSUE_FINDER_TEST_CONFIG_TOKEN")
        .env("ISSUE_FINDER_TEST_TOKEN_SOURCE", source)
        .env("ISSUE_FINDER_TEST_EXPECTED_TOKEN", token);
    command
}

#[cfg(unix)]
#[test]
fn session_reuses_github_cli_without_exposing_credential_output() {
    let fixture = gh_fixture(
        "[ \"$*\" = 'auth token --hostname github.com' ] || exit 2\n\
         [ \"$GH_PROMPT_DISABLED\" = '1' ] || exit 3\n\
         printf 'mock-gh-secret\\n'\n\
         printf 'mock-gh-stderr-secret\\n' >&2",
    );
    let output = probe(fixture.path(), "gh", "mock-gh-secret")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("mock-gh"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("mock-gh"));
}

#[cfg(unix)]
#[test]
fn explicit_credentials_take_precedence_without_invoking_github_cli() {
    let fixture = gh_fixture(": > \"$0.called\"\nexit 1");
    let output = probe(fixture.path(), "env:GITHUB_TOKEN", "env-secret")
        .env("GITHUB_TOKEN", "env-secret")
        .env("ISSUE_FINDER_TEST_CONFIG_TOKEN", "config-secret")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = probe(fixture.path(), "config", "config-secret")
        .env("GITHUB_TOKEN", " ")
        .env("ISSUE_FINDER_TEST_CONFIG_TOKEN", "config-secret")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!fixture.path().join("gh.called").exists());
}

#[cfg(unix)]
#[test]
fn failed_or_malformed_github_cli_auth_is_reported_as_missing() {
    for script in [
        "printf 'failure-secret' >&2; exit 1",
        "printf ' '\nexit 0",
        "printf 'multiple token lines\\n'",
    ] {
        let fixture = gh_fixture(script);
        let output = probe(fixture.path(), "missing", "").output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("failure-secret"));
    }
}

#[test]
fn absent_github_cli_leaves_session_auth_missing() {
    let fixture = TempDir::new().unwrap();
    let output = probe(fixture.path(), "missing", "").output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[cfg(unix)]
#[test]
fn github_cli_auth_timeout_does_not_block_the_agent() {
    let fixture = gh_fixture("exec /bin/sleep 30");
    let started = std::time::Instant::now();
    let output = probe(fixture.path(), "missing", "").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
}

#[test]
fn initialization_neither_prints_nor_persists_inherited_token() {
    let fixture = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
        .arg("init")
        .env("ISSUE_FINDER_HOME", fixture.path())
        .env("GITHUB_TOKEN", "inherited-test-secret")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("inherited-test-secret"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("inherited-test-secret"));
    let saved = fs::read_to_string(fixture.path().join("config.toml")).unwrap();
    assert!(!saved.contains("inherited-test-secret"));
}
