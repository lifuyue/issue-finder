#[path = "support/decision/http.rs"]
mod http;

use std::time::Duration;

use issue_finder::config::Config;
use issue_finder::decision::clef::ClefProvider;
use issue_finder::decision::contract::*;
use issue_finder::decision::evidence::EvidenceSnapshot;
use issue_finder::decision::{questions::SemanticAnswers, request_for};
use issue_finder::github::GitHubIssue;
use issue_finder::github_enrichment::EnrichedIssue;
use serde_json::{json, Map, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const PATH: &str = "/client/v4/accounts/offline-account/ai/run/@cf/cloudflare/clef-flash";
const FAKE_TOKEN: &str = "offline-clef-fake-token";

async fn mock(response: Value) -> (String, JoinHandle<(String, Value)>) {
    http::serve_json(PATH, response).await
}

fn provider(endpoint: String) -> ClefProvider {
    ClefProvider::new(endpoint, FAKE_TOKEN.into(), Duration::from_secs(2)).unwrap()
}

fn choice(id: &str) -> Question {
    Question {
        id: id.into(),
        prompt: "Choose the matching interpretation.".into(),
        criteria: vec!["Use the supplied material; missing evidence remains unknown.".into()],
        context: json!({"content":"Private issue material; do not echo in errors."}),
        kind: QuestionKind::Choice {
            options: vec!["present".into(), "unknown".into()],
        },
    }
}

fn request() -> DecisionRequest {
    DecisionRequest {
        candidate_id: "org/project#1".into(),
        input_id: "offline-material-v1".into(),
        questions: vec![choice("test_question")],
    }
}

fn native_result(request: &DecisionRequest) -> Value {
    let answers: Map<String, Value> = request
        .questions
        .iter()
        .map(|question| {
            let answer = match &question.kind {
                QuestionKind::Boolean => json!({"type":"noul", "noul":0.8}),
                QuestionKind::Choice { options } => {
                    let probabilities: Map<String, Value> = options
                        .iter()
                        .enumerate()
                        .map(|(index, option)| {
                            (option.clone(), json!(if index == 0 { 1.0 } else { 0.0 }))
                        })
                        .collect();
                    json!({"type":"choice", "choice":options[0], "probabilities":probabilities, "confidence":0.94})
                }
                QuestionKind::Score { levels } => {
                    let probabilities: Map<String, Value> = levels
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            (index.to_string(), json!(if index == 1 { 1.0 } else { 0.0 }))
                        })
                        .collect();
                    let legend: Map<String, Value> = levels
                        .iter()
                        .enumerate()
                        .map(|(index, level)| (index.to_string(), json!(level.id)))
                        .collect();
                    json!({"type":"score", "score":1.0, "probabilities":probabilities, "confidence":0.81, "legend":legend})
                }
            };
            (question.id.clone(), answer)
        })
        .collect();
    json!({"model":"clef-flash", "answers":answers, "usage":{"input_tokens":112, "output_tokens":0}})
}

fn envelope(result: Value) -> Value {
    json!({"success":true, "errors":[], "messages":[], "result":result})
}

fn score_question(level_count: usize) -> Question {
    let mut question = choice("severity");
    question.kind = QuestionKind::Score {
        levels: (0..level_count)
            .map(|index| ScoreLevel {
                id: format!("level_{index}"),
                value: 10.0 * index as f64,
            })
            .collect(),
    };
    question
}

#[tokio::test]
async fn seven_business_questions_use_one_native_authenticated_request() {
    let issue = GitHubIssue {
        id: 1,
        number: 1,
        title: "Correct --output example".into(),
        body: "Use --output instead of --out.".into(),
        labels: Vec::new(),
        url: "https://github.com/org/project/issues/1".into(),
        repo_full_name: "org/project".into(),
        repo_name: "project".into(),
        repo_description: String::new(),
        repo_stars: 2000,
        created_at: String::new(),
        updated_at: String::new(),
    };
    let evidence = EvidenceSnapshot::from_issue(&issue, &EnrichedIssue::from_issue(&issue));
    let request = request_for(&evidence, &Config::default().profile);
    assert_eq!(request.questions.len(), 7);
    let native = native_result(&request);
    let (endpoint, server) = mock(envelope(native.clone())).await;
    let response = provider(endpoint).decide(&request).await.unwrap();
    let (headers, sent) = server.await.unwrap();
    assert!(headers.starts_with(&format!("POST {PATH} HTTP/1.1\r\n")));
    assert!(headers
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {FAKE_TOKEN}\r\n")));
    assert_eq!(sent["model"], "clef-flash");
    assert!(sent.get("messages").is_none());
    assert_eq!(sent["state"]["input_id"], request.input_id);
    assert_eq!(sent["questions"].as_object().unwrap().len(), 7);
    for question in &request.questions {
        assert_eq!(
            sent["state"]["question_materials"][&question.id],
            question.context
        );
        let typed = &sent["questions"][&question.id];
        assert_eq!(typed["type"], "choice");
        assert!(typed["instructions"]
            .as_str()
            .unwrap()
            .contains(&question.prompt));
        for criterion in &question.criteria {
            assert!(typed["instructions"].as_str().unwrap().contains(criterion));
        }
    }
    assert_eq!(response.status, ResponseStatus::Complete);
    assert!(SemanticAnswers::from_response(&request, &response).is_ok());
    assert_eq!(response.metadata.provider, "cloudflare_clef_flash");
    assert_eq!(response.metadata.model, "clef-flash");
    assert_eq!(response.metadata.usage, Some(native["usage"].clone()));
    assert_eq!(
        response.metadata.native_answers,
        Some(native["answers"].clone())
    );
    assert_eq!(response.metadata.request_id, None);
    assert_eq!(response.metadata.server_latency_ms, None);
}

#[tokio::test]
async fn cloudflare_native_types_preserve_probabilities_and_confidence() {
    let mut request = request();
    let mut boolean = choice("urgent");
    boolean.kind = QuestionKind::Boolean;
    request.questions.extend([boolean, score_question(3)]);
    let native = native_result(&request);
    let (endpoint, server) = mock(envelope(native.clone())).await;
    let response = provider(endpoint).decide(&request).await.unwrap();
    let (_, sent) = server.await.unwrap();
    assert_eq!(sent["questions"]["urgent"]["type"], "noul");
    assert_eq!(
        sent["questions"]["severity"]["criteria"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(response.answers[1].answer, Some(Answer::Boolean(true)));
    assert_eq!(
        response.answers[1].probabilities,
        Some(ProbabilityInfo::Boolean {
            true_probability: 0.8
        })
    );
    assert_eq!(
        response.answers[2].answer,
        Some(Answer::Score("level_1".into()))
    );
    assert!(matches!(
        response.answers[2].probabilities,
        Some(ProbabilityInfo::Score {
            expected_value: 10.0,
            ..
        })
    ));
    assert_eq!(response.metadata.native_answers.unwrap(), native["answers"]);
}

#[tokio::test]
async fn clef_choice_requires_a_highest_probability_option_and_accepts_ties() {
    let request = request();
    for (probabilities, valid) in [
        (json!({"present":0.2,"unknown":0.8}), false),
        (json!({"present":0.5,"unknown":0.5}), true),
    ] {
        let mut native = native_result(&request);
        native["answers"]["test_question"]["probabilities"] = probabilities;
        let (endpoint, server) = mock(envelope(native)).await;
        let result = provider(endpoint).decide(&request).await;
        if valid {
            assert_eq!(
                result.unwrap().answers[0].answer,
                Some(Answer::Choice("present".into()))
            );
        } else {
            assert_eq!(result.unwrap_err().code, "invalid_provider_response");
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn failed_http_200_envelopes_return_classified_redacted_errors() {
    let cases = [
        (
            "Authentication error: offline-clef-fake-token",
            "authentication_failed",
        ),
        (
            "Forbidden: Workers AI permission missing",
            "permission_denied",
        ),
        ("Free allocation quota exhausted", "quota_exhausted"),
        ("Rate limit exceeded", "rate_limited"),
        ("Model temporarily unavailable", "overloaded"),
        ("Unknown provider failure", "provider_failed"),
    ];
    for (message, expected) in cases {
        let request = request();
        let raw = json!({"success":false, "errors":[{"code":1234, "message":format!("{message}; Private issue material") }], "result":native_result(&request)});
        let (endpoint, server) = mock(raw).await;
        let error = provider(endpoint).decide(&request).await.unwrap_err();
        server.await.unwrap();
        assert_eq!(error.code, expected);
        assert!(!error.to_string().contains(FAKE_TOKEN));
        assert!(!error.to_string().contains("Private issue material"));
        assert!(!error.to_string().contains(message));
    }
}

#[tokio::test]
async fn malformed_envelopes_usage_and_model_confusion_never_produce_answers() {
    let request = request();
    let valid = envelope(native_result(&request));
    let mut cases = vec![native_result(&request)];
    for key in ["success", "errors", "result"] {
        let mut raw = valid.clone();
        raw.as_object_mut().unwrap().remove(key);
        cases.push(raw);
    }
    let mut raw = valid.clone();
    raw["success"] = json!("true");
    cases.push(raw);
    let mut raw = valid.clone();
    raw["errors"] = Value::Null;
    cases.push(raw);
    for model in ["clef", "decision-model-preview", "gpt-6-luna"] {
        let mut raw = valid.clone();
        raw["result"]["model"] = json!(model);
        cases.push(raw);
    }
    for usage in [
        Value::Null,
        json!({"input_tokens":12}),
        json!({"output_tokens":0}),
        json!({"input_tokens":-1,"output_tokens":0}),
        json!({"input_tokens":1.5,"output_tokens":0}),
        json!({"input_tokens":12,"output_tokens":1}),
    ] {
        let mut raw = valid.clone();
        raw["result"]["usage"] = usage;
        cases.push(raw);
    }
    for raw in cases {
        let (endpoint, server) = mock(raw).await;
        assert_eq!(
            provider(endpoint).decide(&request).await.unwrap_err().code,
            "invalid_provider_response"
        );
        server.await.unwrap();
    }
    let mut contradictory = valid;
    contradictory["errors"] = json!([{"message":"Authentication error"}]);
    let (endpoint, server) = mock(contradictory).await;
    assert_eq!(
        provider(endpoint).decide(&request).await.unwrap_err().code,
        "authentication_failed"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn clef_input_limits_are_checked_before_network_admission() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(format!("http://{}{PATH}", listener.local_addr().unwrap()));
    let mut cases = Vec::new();
    let mut too_many = request();
    too_many.questions = (0..65).map(|index| choice(&format!("q{index}"))).collect();
    cases.push(too_many);
    for id in [
        "x".repeat(101),
        "含中文".into(),
        "illegal/id".into(),
        "illegal id".into(),
    ] {
        let mut invalid = request();
        invalid.questions[0].id = id;
        cases.push(invalid);
    }
    let mut missing_instructions = request();
    missing_instructions.questions[0].prompt.clear();
    cases.push(missing_instructions);
    let mut many_options = request();
    many_options.questions[0].kind = QuestionKind::Choice {
        options: (0..256).map(|index| format!("option_{index}")).collect(),
    };
    cases.push(many_options);
    let mut many_levels = request();
    many_levels.questions[0] = score_question(11);
    cases.push(many_levels);
    for invalid in cases {
        assert_eq!(
            provider.decide(&invalid).await.unwrap_err().code,
            "invalid_contract"
        );
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn inclusive_clef_input_boundaries_are_accepted() {
    let mut request = request();
    request.questions = (0..62).map(|index| choice(&format!("q{index}"))).collect();
    let mut large_choice = choice(&"a".repeat(100));
    large_choice.kind = QuestionKind::Choice {
        options: (0..255).map(|index| format!("option_{index}")).collect(),
    };
    request.questions.extend([large_choice, score_question(10)]);
    let (endpoint, server) = mock(envelope(native_result(&request))).await;
    assert_eq!(
        provider(endpoint)
            .decide(&request)
            .await
            .unwrap()
            .answers
            .len(),
        64
    );
    let (_, sent) = server.await.unwrap();
    assert_eq!(sent["questions"].as_object().unwrap().len(), 64);
}

#[test]
fn configuration_requires_the_fixed_clef_flash_route_and_credentials() {
    let base = "https://api.cloudflare.com";
    for path in [
        "/client/v4/accounts/offline-account/ai/run/@cf/cloudflare/clef",
        "/client/v4/accounts/offline-account/ai/run/@cf/meta/chat",
        "/client/v4/accounts//ai/run/@cf/cloudflare/clef-flash",
        "/client/v4/accounts/offline-account/ai/run/@cf/cloudflare/clef-flash/",
        "/v1/chat/completions",
    ] {
        assert!(ClefProvider::new(
            format!("{base}{path}"),
            FAKE_TOKEN.into(),
            Duration::from_secs(2)
        )
        .is_err());
    }
    for endpoint in [
        format!("http://api.cloudflare.com{PATH}"),
        format!("{base}{PATH}?model=clef"),
        format!("{base}{PATH}#fragment"),
        format!("https://fake-token@api.cloudflare.com{PATH}"),
    ] {
        assert!(ClefProvider::new(endpoint, FAKE_TOKEN.into(), Duration::from_secs(2)).is_err());
    }
    assert!(
        ClefProvider::new(format!("{base}{PATH}"), " ".into(), Duration::from_secs(2)).is_err()
    );
    assert!(ClefProvider::new(format!("{base}{PATH}"), FAKE_TOKEN.into(), Duration::ZERO).is_err());
}

#[test]
fn fingerprint_identifies_provider_route_and_timeout_without_credentials() {
    let endpoint = format!("https://api.cloudflare.com{PATH}");
    let first = provider(endpoint.clone()).fingerprint();
    let rotated = ClefProvider::new(
        endpoint.clone(),
        "rotated-fake-token".into(),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(first, rotated.fingerprint());
    assert_ne!(
        first,
        provider(endpoint.replace("offline-account", "another-account")).fingerprint()
    );
    let longer = ClefProvider::new(endpoint, FAKE_TOKEN.into(), Duration::from_secs(3)).unwrap();
    assert_ne!(first, longer.fingerprint());
    assert!(first.contains("clef-flash"));
    assert!(!first.contains(FAKE_TOKEN));
    assert!(!first.contains("offline-account"));
}
