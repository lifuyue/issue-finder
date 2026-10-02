use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use issue_finder::config::{Config, DecisionConfig, DecisionProvider};
use serde_json::{json, Value};

struct Fixture {
    home: tempfile::TempDir,
}

impl Fixture {
    fn new(config: &Config) -> Self {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            toml::to_string(config).unwrap(),
        )
        .unwrap();
        Self { home }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_issue-finder"));
        command
            .env_clear()
            .env("ISSUE_FINDER_HOME", self.home.path());
        command.arg("decision-check");
        command
    }
}

fn result(output: Output, success: bool) -> Value {
    assert_eq!(
        output.status.success(),
        success,
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic-selection-key"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("synthetic-selection-key"));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["success"], success);
    value
}

#[test]
fn default_is_native_aliyun_and_other_provider_names_are_explicit() {
    assert_eq!(
        DecisionConfig::default().provider,
        DecisionProvider::AliyunDecision
    );
    assert_eq!(
        toml::from_str::<DecisionConfig>("concurrency = 12")
            .unwrap()
            .provider,
        DecisionProvider::AliyunDecision
    );
    for (name, expected) in [
        ("aliyun_decision", DecisionProvider::AliyunDecision),
        (
            "cloudflare_clef_flash",
            DecisionProvider::CloudflareClefFlash,
        ),
        ("codex", DecisionProvider::Codex),
    ] {
        let config: DecisionConfig =
            toml::from_str(&format!("provider = '{name}'\nconcurrency = 17")).unwrap();
        assert_eq!(config.provider, expected);
        config.validate().unwrap();
    }
    assert!(toml::from_str::<DecisionConfig>("provider = 'qwen-json'").is_err());
    assert!(
        toml::from_str::<DecisionConfig>("[aliyun_decision]\napi_key = 'do-not-store-keys'")
            .is_err()
    );
}

#[test]
fn legacy_config_and_cli_alias_select_the_same_provider_and_write_canonical_config() {
    let mut config = Config::default();
    config.decision.provider = DecisionProvider::CloudflareClefFlash;
    config.decision.concurrency = 17;
    config.decision.cloudflare_clef_flash.account_id = "synthetic-account".into();
    let canonical = toml::to_string(&config).unwrap();
    assert!(canonical.contains("[decision]"));
    assert!(!canonical.contains("[system1]"));
    let legacy = canonical.replace("[decision", "[system1");
    let loaded: Config = toml::from_str(&legacy).unwrap();
    assert_eq!(loaded.decision.provider, config.decision.provider);
    assert_eq!(loaded.decision.concurrency, 17);
    assert_eq!(toml::to_string(&loaded).unwrap(), canonical);
    let fixture = Fixture::new(&config);
    fs::write(fixture.home.path().join("config.toml"), legacy).unwrap();
    for command in ["decision-check", "system1-check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_issue-finder"))
            .env_clear()
            .env("ISSUE_FINDER_HOME", fixture.home.path())
            .arg(command)
            .output()
            .unwrap();
        let output = result(output, false);
        assert!(output["error"]
            .as_str()
            .unwrap()
            .contains("CLOUDFLARE_API_TOKEN"));
    }
}

#[test]
fn missing_native_setup_never_silently_uses_codex() {
    let mut config = Config::default();
    config.decision.codex_binary = "/synthetic/missing-codex".into();
    let fixture = Fixture::new(&config);
    let output = result(fixture.command().output().unwrap(), false);
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT"));

    let output = result(
        fixture
            .command()
            .env(
                "ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT",
                "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone",
            )
            .output()
            .unwrap(),
        false,
    );
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("DASHSCOPE_API_KEY"));

    let output = result(
        fixture
            .command()
            .args(["--provider", "cloudflare-clef-flash"])
            .env("CLOUDFLARE_ACCOUNT_ID", "synthetic-account")
            .output()
            .unwrap(),
        false,
    );
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("CLOUDFLARE_API_TOKEN"));

    let output = result(
        fixture
            .command()
            .args([
                "--provider",
                "aliyun-decision",
                "--codex-binary",
                "/synthetic/missing-codex",
            ])
            .output()
            .unwrap(),
        false,
    );
    assert!(output["error"]
        .as_str()
        .unwrap()
        .contains("cannot be combined"));
}

fn mock(model: &'static str, cloudflare: bool) -> (String, thread::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "CLI did not call configured provider"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0; 4096];
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(i) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-selection-key"));
        let length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while bytes.len() < header_end + length {
            let mut buffer = [0; 4096];
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        let request: Value =
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
        let native = json!({"model":model,"answers":{"health":{"type":"choice","choice":"ready","confidence":0.99,"probabilities":{"ready":0.99,"unavailable":0.01}}},"usage":{"input_tokens":25,"output_tokens":0}});
        let response = if cloudflare {
            json!({"success":true,"errors":[],"result":native})
        } else {
            native
        };
        let body = response.to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        request
    });
    let path = if cloudflare {
        "/client/v4/accounts/synthetic-account/ai/run/@cf/cloudflare/clef-flash"
    } else {
        "/compatible-mode/v1/systemone"
    };
    (format!("http://{address}{path}"), server)
}

#[test]
fn cli_uses_selected_native_provider_and_returns_one_json_object() {
    for (cloudflare, credential_name) in [
        (false, None),
        (true, None),
        (true, Some("cf-fallback-1")),
        (true, Some("CF_FALLBACK_2")),
    ] {
        let model = if cloudflare {
            "clef-flash"
        } else {
            "decision-model-preview"
        };
        let (endpoint, server) = mock(model, cloudflare);
        let mut config = Config::default();
        config.decision.codex_binary = "/synthetic/missing-codex".into();
        if cloudflare {
            config.decision.provider = DecisionProvider::CloudflareClefFlash;
            config.decision.cloudflare_clef_flash.endpoint = endpoint.clone();
        } else {
            // Environment endpoint must override the configured workspace URL.
            config.decision.aliyun_decision.endpoint =
                "https://unused.invalid/compatible-mode/v1/systemone".into();
        }
        let fixture = Fixture::new(&config);
        let mut command = fixture.command();
        if cloudflare {
            if let Some(name) = credential_name {
                command
                    .env("ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV", name)
                    .env(name, "synthetic-selection-key")
                    .env("ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV", "cf-fallback-id")
                    .env("cf-fallback-id", "synthetic-account")
                    .env("CLOUDFLARE_ACCOUNT_ID", "wrong-default-account")
                    .env("CLOUDFLARE_API_TOKEN", "wrong-default-key");
            } else {
                command.env("CLOUDFLARE_API_TOKEN", "synthetic-selection-key");
            }
        } else {
            command
                .env("DASHSCOPE_API_KEY", "synthetic-selection-key")
                // A Cloudflare-only selector cannot change Alibaba authentication.
                .env("ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV", "missing-cf-key")
                .env("ISSUE_FINDER_ALIYUN_DECISION_ENDPOINT", endpoint);
        }
        let output = result(command.output().unwrap(), true);
        let request = server.join().unwrap();
        assert_eq!(request["model"], model);
        assert_eq!(output["response"]["metadata"]["model"], model);
        assert_eq!(
            output["response"]["metadata"]["provider"],
            if cloudflare {
                "cloudflare_clef_flash"
            } else {
                "aliyun_decision"
            }
        );
        assert_eq!(
            output["response"]["answers"][0]["probabilities"]["type"],
            "choice"
        );
        assert!(output["providerFingerprint"]
            .as_str()
            .unwrap()
            .contains("systemone-v1"));
    }
}

#[test]
fn explicit_cloudflare_credential_never_falls_back_or_echoes_invalid_selector() {
    let mut config = Config::default();
    config.decision.provider = DecisionProvider::CloudflareClefFlash;
    config.decision.cloudflare_clef_flash.account_id = "synthetic-account".into();
    let fixture = Fixture::new(&config);
    for (name, value) in [
        ("cf-fallback-1", None),
        ("cf-fallback-1", Some("")),
        ("cf-fallback-1", Some("  ")),
        ("", None),
        ("bad=name", None),
        ("Bearer synthetic-selection-key", None),
    ] {
        let mut command = fixture.command();
        command
            .env("CLOUDFLARE_API_TOKEN", "synthetic-selection-key")
            .env("cf-fallback-2", "synthetic-selection-key")
            .env("ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV", "cf-fallback-id")
            .env("cf-fallback-id", "synthetic-account")
            .env("ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV", name);
        if let Some(value) = value {
            command.env(name, value);
        }
        let output = result(command.output().unwrap(), false);
        let error = output["error"].as_str().unwrap();
        assert!(error.contains(if name == "cf-fallback-1" {
            "cf-fallback-1 is missing or empty"
        } else {
            "ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV must name"
        }));
    }
}

#[test]
fn manual_cloudflare_selection_requires_complete_account_pair_before_network() {
    let mut config = Config::default();
    config.decision.provider = DecisionProvider::CloudflareClefFlash;
    config.decision.cloudflare_clef_flash.account_id = "default-account".into();
    // A stale account-specific endpoint must not receive the new account's key.
    config.decision.cloudflare_clef_flash.endpoint = "https://unused.invalid/client/v4/accounts/default-account/ai/run/@cf/cloudflare/clef-flash".into();
    let fixture = Fixture::new(&config);
    for (token_selector, account_selector, account_value, expected) in [
        (Some("cf-api"), None, Some("default-account"), "Set both"),
        (None, Some("cf-id"), Some("default-account"), "Set both"),
        (
            Some("cf-api"),
            Some("cf-id"),
            None,
            "nonempty valid account ID",
        ),
        (
            Some("cf-api"),
            Some("cf-id"),
            Some("  "),
            "nonempty valid account ID",
        ),
        (
            Some("cf-api"),
            Some("cf-id"),
            Some("bad/account"),
            "nonempty valid account ID",
        ),
        (
            Some("cf-api"),
            Some(""),
            None,
            "ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV must name",
        ),
        (
            Some("cf-api"),
            Some("bad=name"),
            None,
            "ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV must name",
        ),
        (
            Some("cf-api"),
            Some("cf-id"),
            Some("other-account"),
            "endpoint does not match",
        ),
    ] {
        let mut command = fixture.command();
        command
            .env("CLOUDFLARE_API_TOKEN", "synthetic-selection-key")
            .env("CLOUDFLARE_ACCOUNT_ID", "default-account")
            .env("cf-api", "synthetic-selection-key");
        if let Some(name) = token_selector {
            command.env("ISSUE_FINDER_CLOUDFLARE_API_TOKEN_ENV", name);
        }
        if let Some(name) = account_selector {
            command.env("ISSUE_FINDER_CLOUDFLARE_ACCOUNT_ID_ENV", name);
        }
        if let Some(value) = account_value {
            command.env("cf-id", value);
        }
        let output = result(command.output().unwrap(), false);
        assert!(
            output["error"].as_str().unwrap().contains(expected),
            "{output}"
        );
    }
}
