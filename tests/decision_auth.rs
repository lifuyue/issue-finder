#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};
use tempfile::TempDir;

const AUTH_ENV: &str = "ISSUE_FINDER_CODEX_AUTH_JSON";
const HOME_ENV: &str = "ISSUE_FINDER_CODEX_HOME";

fn auth(label: &str) -> Value {
    json!({
        "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": format!("SYNTHETIC_AUTH_ID_{label}"),
            "access_token": format!("SYNTHETIC_AUTH_ACCESS_{label}"),
            "refresh_token": format!("SYNTHETIC_AUTH_REFRESH_{label}"),
            "account_id": "synthetic-account"
        },
        "last_refresh": "2026-10-01T00:00:00Z"
    })
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn write_private(path: &Path, value: &Value) {
    fs::write(path, value.to_string()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

struct Fixture {
    directory: TempDir,
    state: PathBuf,
    host_home: PathBuf,
    main_codex_home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("issue-finder");
        let host_home = directory.path().join("host-home");
        let main_codex_home = directory.path().join("main-agent-codex");
        private_dir(&host_home);
        private_dir(&main_codex_home);
        write_private(&main_codex_home.join("auth.json"), &auth("MAIN_AGENT"));
        private_dir(&host_home.join(".codex"));
        write_private(&host_home.join(".codex/auth.json"), &auth("HOST_LOGIN"));
        Self {
            directory,
            state,
            host_home,
            main_codex_home,
        }
    }

    fn home(&self) -> PathBuf {
        self.state.join("system1/codex-home")
    }

    fn command(&self, subcommand: &str) -> Command {
        let mut command = self.base_command(Path::new(env!("CARGO_BIN_EXE_issue-finder")));
        command.arg(subcommand);
        command
    }

    fn base_command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        // No inherited credentials or configuration can reach this offline fixture.
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.host_home)
            .env("CODEX_HOME", &self.main_codex_home)
            .env("ISSUE_FINDER_HOME", &self.state)
            .env("TMPDIR", self.directory.path());
        command
    }

    fn initialize(&self, value: &Value, replace: bool) -> Output {
        let mut command = self.command("decision-auth-init");
        command.env(AUTH_ENV, value.to_string());
        if replace {
            command.arg("--replace");
        }
        command.output().unwrap()
    }

    fn assert_host_unchanged(&self) {
        assert_eq!(read_auth(&self.main_codex_home), auth("MAIN_AGENT"));
        assert_eq!(
            read_auth(&self.host_home.join(".codex")),
            auth("HOST_LOGIN")
        );
    }

    fn fake_codex(&self) -> PathBuf {
        let binary = self.directory.path().join("fake-codex");
        fs::write(&binary, FAKE_CODEX).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        binary
    }
}

fn read_auth(home: &Path) -> Value {
    serde_json::from_slice(&fs::read(home.join("auth.json")).unwrap()).unwrap()
}

fn output_json(output: &Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for stream in [&output.stdout, &output.stderr] {
        assert!(
            !String::from_utf8_lossy(stream).contains("SYNTHETIC_AUTH_"),
            "credential content leaked into command output"
        );
    }
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.is_object(), "CLI stdout must be one JSON object");
    assert_eq!(value["success"], success);
    value
}

fn assert_no_staging_files(home: &Path) {
    for entry in fs::read_dir(home).unwrap() {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        assert!(!(name.starts_with(".auth-") && name.ends_with(".tmp")));
    }
}

#[test]
fn initialization_creates_private_default_home_without_leaking_credentials() {
    let fixture = Fixture::new();
    let value = auth("SEED");
    let result = output_json(&fixture.initialize(&value, false), true);
    assert_eq!(result["status"], "initialized");
    assert_eq!(result["authenticationVerified"], false);
    assert_eq!(
        result["codexHome"],
        fixture.home().to_string_lossy().as_ref()
    );
    assert_eq!(read_auth(&fixture.home()), value);
    assert_eq!(fs::metadata(fixture.home()).unwrap().mode() & 0o777, 0o700);
    let metadata = fs::metadata(fixture.home().join("auth.json")).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert_no_staging_files(&fixture.home());
    fixture.assert_host_unchanged();
    // The legacy command must reuse the existing private state directory.
    let alias = fixture
        .command("system1-auth-init")
        .env(AUTH_ENV, auth("DIFFERENT_SEED").to_string())
        .output()
        .unwrap();
    let alias = output_json(&alias, true);
    assert_eq!(alias["codexHome"], result["codexHome"]);
    assert_eq!(read_auth(&fixture.home()), value);
    assert_no_staging_files(&fixture.home());
}

#[test]
fn explicit_home_preserves_refreshed_auth_until_replace_is_requested() {
    let fixture = Fixture::new();
    let home = fixture.directory.path().join("custom-codex");
    let run = |replace: bool| {
        let mut command = fixture.command("decision-auth-init");
        command
            .env(HOME_ENV, &home)
            .env(AUTH_ENV, auth("SEED").to_string());
        if replace {
            command.arg("--replace");
        }
        output_json(&command.output().unwrap(), true)
    };
    assert_eq!(run(false)["status"], "initialized");
    let refreshed = auth("REFRESHED");
    write_private(&home.join("auth.json"), &refreshed);
    assert_eq!(run(false)["status"], "existing_preserved");
    assert_eq!(read_auth(&home), refreshed);
    assert_eq!(run(true)["status"], "initialized");
    assert_eq!(read_auth(&home), auth("SEED"));
    assert!(!fixture.home().exists());
    assert_no_staging_files(&home);
    fixture.assert_host_unchanged();
}

#[test]
fn invalid_or_incoherent_imports_are_rejected_without_outputting_the_input() {
    let mut mixed = auth("MIXED");
    mixed["OPENAI_API_KEY"] = json!("SYNTHETIC_AUTH_API_MIXED");
    let cases = [
        "{SYNTHETIC_AUTH_BROKEN_JSON".into(),
        "\"SYNTHETIC_AUTH_STRING\"".into(),
        "[]".into(),
        " ".into(),
        json!({"auth_mode":"chatgptAuthTokens", "tokens":auth("EXTERNAL")["tokens"]}).to_string(),
        json!({"auth_mode":"agentIdentity", "agent_identity":"SYNTHETIC_AUTH_AGENT"}).to_string(),
        json!({"auth_mode":"SYNTHETIC_AUTH_UNSUPPORTED"}).to_string(),
        json!({"auth_mode":7, "OPENAI_API_KEY":"SYNTHETIC_AUTH_API"}).to_string(),
        mixed.to_string(),
        json!({"auth_mode":"apikey", "OPENAI_API_KEY":"SYNTHETIC_AUTH_API", "tokens":{"access_token":"SYNTHETIC_AUTH_PARTIAL"}}).to_string(),
        json!({"OPENAI_API_KEY":"SYNTHETIC_AUTH_API", "tokens":{"refresh_token":"SYNTHETIC_AUTH_PARTIAL"}}).to_string(),
        json!({"auth_mode":"chatgpt", "tokens":{"access_token":"SYNTHETIC_AUTH_PARTIAL"}}).to_string(),
        json!({}).to_string(),
    ];
    for raw in cases {
        let fixture = Fixture::new();
        let output = fixture
            .command("decision-auth-init")
            .env(AUTH_ENV, raw)
            .output()
            .unwrap();
        output_json(&output, false);
        assert!(!fixture.home().join("auth.json").exists());
        fixture.assert_host_unchanged();
    }
}

#[test]
fn api_key_and_legacy_chatgpt_files_are_supported() {
    let mut legacy = auth("LEGACY");
    legacy.as_object_mut().unwrap().remove("auth_mode");
    for value in [
        legacy,
        json!({"auth_mode":"apikey", "OPENAI_API_KEY":"SYNTHETIC_AUTH_API"}),
    ] {
        let fixture = Fixture::new();
        output_json(&fixture.initialize(&value, false), true);
        assert_eq!(read_auth(&fixture.home()), value);
    }
}

#[test]
fn missing_secret_and_relative_home_fail_before_creating_auth() {
    let fixture = Fixture::new();
    output_json(
        &fixture.command("decision-auth-init").output().unwrap(),
        false,
    );
    let output = fixture
        .command("decision-auth-init")
        .env(AUTH_ENV, auth("SEED").to_string())
        .env(HOME_ENV, "relative-codex-home")
        .output()
        .unwrap();
    output_json(&output, false);
    assert!(!fixture.home().exists());
}

#[test]
fn initialization_cannot_target_the_main_agent_or_default_host_login() {
    let fixture = Fixture::new();
    let bypass = fixture.main_codex_home.join("nonexistent/..");
    for home in [
        &fixture.main_codex_home,
        &fixture.host_home.join(".codex"),
        &bypass,
    ] {
        let output = fixture
            .command("decision-auth-init")
            .arg("--replace")
            .env(HOME_ENV, home)
            .env(AUTH_ENV, auth("REPLACEMENT").to_string())
            .output()
            .unwrap();
        output_json(&output, false);
        fixture.assert_host_unchanged();
    }
    assert!(!fixture.main_codex_home.join("nonexistent").exists());
}

#[test]
fn symlinked_home_and_auth_file_cannot_redirect_import_or_replacement() {
    let fixture = Fixture::new();
    let link = fixture.directory.path().join("home-link");
    let target = fixture.directory.path().join("symlink-target");
    private_dir(&target);
    write_private(&target.join("auth.json"), &auth("TARGET"));
    symlink(&target, &link).unwrap();
    let output = fixture
        .command("decision-auth-init")
        .arg("--replace")
        .env(HOME_ENV, &link)
        .env(AUTH_ENV, auth("REPLACEMENT").to_string())
        .output()
        .unwrap();
    output_json(&output, false);
    assert_eq!(read_auth(&target), auth("TARGET"));
    private_dir(&fixture.home());
    symlink(
        fixture.main_codex_home.join("auth.json"),
        fixture.home().join("auth.json"),
    )
    .unwrap();
    for replace in [false, true] {
        output_json(&fixture.initialize(&auth("REPLACEMENT"), replace), false);
        assert!(fs::symlink_metadata(fixture.home().join("auth.json"))
            .unwrap()
            .file_type()
            .is_symlink());
        fixture.assert_host_unchanged();
    }
}

#[test]
fn overly_broad_permissions_and_hardlinked_auth_are_rejected() {
    for broad_home in [true, false] {
        let fixture = Fixture::new();
        private_dir(&fixture.home());
        write_private(&fixture.home().join("auth.json"), &auth("EXISTING"));
        if broad_home {
            fs::set_permissions(fixture.home(), fs::Permissions::from_mode(0o750)).unwrap();
        } else {
            fs::set_permissions(
                fixture.home().join("auth.json"),
                fs::Permissions::from_mode(0o640),
            )
            .unwrap();
        }
        output_json(&fixture.initialize(&auth("REPLACEMENT"), true), false);
        assert_eq!(read_auth(&fixture.home()), auth("EXISTING"));
    }
    let fixture = Fixture::new();
    private_dir(&fixture.home());
    fs::hard_link(
        fixture.main_codex_home.join("auth.json"),
        fixture.home().join("auth.json"),
    )
    .unwrap();
    output_json(&fixture.initialize(&auth("REPLACEMENT"), true), false);
    fixture.assert_host_unchanged();
}

#[test]
fn missing_runtime_auth_never_falls_back_to_the_host_or_autoimports_secret() {
    let fixture = Fixture::new();
    let binary = fixture.fake_codex();
    for existing_home in [false, true] {
        if existing_home {
            private_dir(&fixture.home());
        }
        let output = fixture
            .command("decision-check")
            .arg("--codex-binary")
            .arg(&binary)
            .env(AUTH_ENV, auth("SEED").to_string())
            .output()
            .unwrap();
        let result = output_json(&output, false);
        assert!(result["error"]
            .as_str()
            .unwrap()
            .contains("decision-auth-init"));
        assert!(!fixture.home().join("auth.json").exists());
        assert!(
            !binary.with_extension("observed").exists(),
            "no Codex process may start without runtime auth"
        );
        fixture.assert_host_unchanged();
    }
}

#[test]
fn fake_codex_receives_only_isolated_file_auth_and_keeps_proxy_and_ca_settings() {
    let fixture = Fixture::new();
    output_json(&fixture.initialize(&auth("SEED"), false), true);
    let binary = fixture.fake_codex();
    // A synthetic changed seed must not replace the initialized runtime credential.
    let output = fixture
        .command("decision-check")
        .arg("--codex-binary")
        .arg(&binary)
        .env(AUTH_ENV, auth("UNIMPORTED").to_string())
        .env("CODEX_ACCESS_TOKEN", "SYNTHETIC_AUTH_COMPETING_ACCESS")
        .env("CODEX_API_KEY", "SYNTHETIC_AUTH_COMPETING_CODEX_KEY")
        .env("OPENAI_API_KEY", "SYNTHETIC_AUTH_COMPETING_API_KEY")
        .env("HTTP_PROXY", "http://synthetic-proxy.invalid:8080")
        .env("HTTPS_PROXY", "http://synthetic-proxy.invalid:8080")
        .env("NO_PROXY", "localhost,127.0.0.1")
        .env("SSL_CERT_FILE", "/synthetic/ca.pem")
        .env("CODEX_CA_CERTIFICATE", "/synthetic/codex-ca.pem")
        .output()
        .unwrap();
    output_json(&output, true);
    let observed: Vec<Value> = fs::read_to_string(binary.with_extension("observed"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        observed.len() >= 3,
        "discovery and app-server children must be checked"
    );
    for child in &observed {
        let env = &child["environment"];
        assert_eq!(env["CODEX_HOME"], fixture.home().to_string_lossy().as_ref());
        for key in [
            AUTH_ENV,
            "CODEX_ACCESS_TOKEN",
            "CODEX_API_KEY",
            "OPENAI_API_KEY",
        ] {
            assert_eq!(env[key], Value::Null, "child inherited {key}");
        }
        assert_eq!(env["HTTP_PROXY"], "http://synthetic-proxy.invalid:8080");
        assert_eq!(env["HTTPS_PROXY"], "http://synthetic-proxy.invalid:8080");
        assert_eq!(env["NO_PROXY"], "localhost,127.0.0.1");
        assert_eq!(env["SSL_CERT_FILE"], "/synthetic/ca.pem");
        assert_eq!(env["CODEX_CA_CERTIFICATE"], "/synthetic/codex-ca.pem");
    }
    let server = observed
        .iter()
        .find(|child| {
            let args = child["args"].as_array().unwrap();
            args.iter().any(|arg| arg == "app-server") && !args.iter().any(|arg| arg == "--help")
        })
        .unwrap();
    assert!(server["args"]
        .as_array()
        .unwrap()
        .windows(2)
        .any(|args| args == [json!("-c"), json!("cli_auth_credentials_store=\"file\"")]));
    assert_eq!(read_auth(&fixture.home()), auth("SEED"));
    fixture.assert_host_unchanged();
}

#[test]
fn initialized_explicit_home_works_without_reinjecting_the_seed_secret() {
    let fixture = Fixture::new();
    output_json(&fixture.initialize(&auth("SEED"), false), true);
    let binary = fixture.fake_codex();
    let output = fixture
        .command("decision-check")
        .arg("--codex-binary")
        .arg(binary)
        .env(HOME_ENV, fixture.home())
        .output()
        .unwrap();
    output_json(&output, true);
    assert_eq!(read_auth(&fixture.home()), auth("SEED"));
    fixture.assert_host_unchanged();
}

#[test]
fn concurrent_initializers_publish_one_complete_seed_without_clobbering() {
    let fixture = Fixture::new();
    let mut children = Vec::new();
    for number in 0..6 {
        let label = number.to_string();
        let child = fixture
            .command("decision-auth-init")
            .env(AUTH_ENV, auth(&label).to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        children.push((label, child));
    }
    let mut winner = None;
    for (label, child) in children {
        let result = output_json(&child.wait_with_output().unwrap(), true);
        match result["status"].as_str().unwrap() {
            "initialized" => assert!(
                winner.replace(label).is_none(),
                "two initializers overwrote auth"
            ),
            "existing_preserved" => {}
            status => panic!("unexpected initialization status: {status}"),
        }
    }
    assert_eq!(read_auth(&fixture.home()), auth(&winner.unwrap()));
    assert_no_staging_files(&fixture.home());
    fixture.assert_host_unchanged();
}

#[test]
fn wrapper_explicitly_initializes_auth_and_keeps_diagnostic_stdout_one_json_object() {
    let fixture = Fixture::new();
    let binary = fixture.fake_codex();
    let home = fixture.directory.path().join("wrapper-home");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/decision-codex.sh");
    let output = fixture
        .base_command(Path::new("/bin/bash"))
        .arg(script)
        .args(["--check-only", "--auth-from-env", "--issue-finder-binary"])
        .arg(env!("CARGO_BIN_EXE_issue-finder"))
        .arg("--codex-binary")
        .arg(&binary)
        .env(AUTH_ENV, auth("WRAPPER").to_string())
        .env(HOME_ENV, &home)
        .output()
        .unwrap();
    output_json(&output, true);
    assert!(String::from_utf8_lossy(&output.stderr).contains("initialized"));
    assert_eq!(read_auth(&home), auth("WRAPPER"));
    let observed: Vec<Value> = fs::read_to_string(binary.with_extension("observed"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        observed
            .iter()
            .any(|child| child["environment"]["CODEX_HOME"]
                == fixture.main_codex_home.to_string_lossy().as_ref()),
        "wrapper version inspection should not change its inherited home"
    );
    for child in &observed {
        assert_eq!(child["environment"][AUTH_ENV], Value::Null);
    }
    let server = observed
        .iter()
        .find(|child| {
            let args = child["args"].as_array().unwrap();
            args.iter().any(|arg| arg == "app-server") && !args.iter().any(|arg| arg == "--help")
        })
        .unwrap();
    assert_eq!(
        server["environment"]["CODEX_HOME"],
        home.to_string_lossy().as_ref()
    );
    fixture.assert_host_unchanged();
}

#[test]
fn legacy_wrapper_missing_auth_json_stops_before_codex_or_model_verification() {
    let fixture = Fixture::new();
    let binary = fixture.fake_codex();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/system1-codex.sh");
    let output = fixture
        .base_command(Path::new("/bin/bash"))
        .arg(script)
        .args(["--check-only", "--auth-from-env", "--issue-finder-binary"])
        .arg(env!("CARGO_BIN_EXE_issue-finder"))
        .arg("--codex-binary")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let failure: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(failure["success"], false);
    assert!(failure["error"].as_str().unwrap().contains(AUTH_ENV));
    assert!(!binary.with_extension("observed").exists());
    assert!(!fixture.home().exists());
    fixture.assert_host_unchanged();
}

const FAKE_CODEX: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys

keys = ['CODEX_HOME', 'ISSUE_FINDER_CODEX_AUTH_JSON', 'CODEX_ACCESS_TOKEN',
        'CODEX_API_KEY', 'OPENAI_API_KEY', 'HTTP_PROXY', 'HTTPS_PROXY',
        'NO_PROXY', 'SSL_CERT_FILE', 'CODEX_CA_CERTIFICATE']
observed = pathlib.Path(sys.argv[0]).with_suffix('.observed')
with observed.open('a') as log:
    log.write(json.dumps({'args': sys.argv[1:],
                         'environment': {key: os.environ.get(key) for key in keys}}) + '\n')
if '--version' in sys.argv:
    print('codex-cli 0.159.3')
    sys.exit(0)
if '--help' in sys.argv:
    sys.exit(0)

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    message = json.loads(line)
    method = message.get('method')
    if method in ('initialize', 'thread/unsubscribe', 'turn/interrupt'):
        send({'id': message['id'], 'result': {}})
    elif method == 'thread/start':
        send({'id': message['id'], 'result': {'model': 'gpt-6-luna',
              'reasoningEffort': 'none', 'thread': {'id': 'thread-1'}}})
    elif method == 'turn/start':
        request = json.loads(message['params']['input'][0]['text'])
        answer = {'candidate_id': request['candidate_id'], 'input_id': request['input_id'],
                  'answers': [{'question_id': request['questions'][0]['id'],
                  'status': 'answered', 'answer': {'type': 'choice', 'value': 'ready'}}]}
        send({'id': message['id'], 'result': {'turn': {'id': 'turn-1'}}})
        send({'method': 'item/completed', 'params': {'threadId': 'thread-1',
              'turnId': 'turn-1', 'item': {'type': 'agentMessage',
              'phase': 'final_answer', 'text': json.dumps(answer)}}})
        send({'method': 'turn/completed', 'params': {'threadId': 'thread-1',
              'turnId': 'turn-1', 'turn': {'id': 'turn-1', 'status': 'completed'}}})
"#;
