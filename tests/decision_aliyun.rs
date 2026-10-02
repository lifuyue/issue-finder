#[path = "support/decision/http.rs"]
mod http;

use std::time::Duration;

use issue_finder::decision::aliyun::{AliyunProvider, MODEL};
use issue_finder::decision::contract::*;
use serde_json::{json, Value};
use tokio::task::JoinHandle;

const API_KEY: &str = "synthetic-aliyun-test-key";

async fn server(response: Value) -> (String, JoinHandle<(String, Value)>) {
    http::serve_json("/compatible-mode/v1/systemone", response).await
}

fn request() -> DecisionRequest {
    let question = |id: &str, kind| Question {
        id: id.into(),
        prompt: format!("Decide {id} from this question's material."),
        criteria: vec![
            format!("First criterion for {id}; interpret negation and quotations."),
            format!("Second criterion for {id}; preserve missing evidence."),
        ],
        context: json!({"material_for":id, "text":format!("Private material for {id}")}),
        kind,
    };
    DecisionRequest {
        candidate_id: "org/repo:42".into(),
        input_id: "immutable-input-id".into(),
        questions: vec![
            question(
                "task_type",
                QuestionKind::Choice {
                    options: vec!["concrete_change".into(), "unclear".into()],
                },
            ),
            question("escalate", QuestionKind::Boolean),
            question(
                "severity",
                QuestionKind::Score {
                    levels: vec![
                        ScoreLevel {
                            id: "low".into(),
                            value: 0.0,
                        },
                        ScoreLevel {
                            id: "medium".into(),
                            value: 10.0,
                        },
                        ScoreLevel {
                            id: "high".into(),
                            value: 100.0,
                        },
                    ],
                },
            ),
        ],
    }
}

fn response() -> Value {
    json!({
        "model":MODEL,
        "request_id":"native-request-42",
        "latency_ms":52.9,
        "usage":{"input_tokens":125},
        "answers":{
            "task_type":{"type":"choice", "choice":"concrete_change", "confidence":0.88,
                "probabilities":{"concrete_change":0.94,"unclear":0.06}},
            "escalate":{"type":"noul", "noul":0.5},
            "severity":{"type":"score", "score":1.08, "confidence":0.91,
                "legend":{"0":"low (business value 0)", "1":"medium (business value 10)", "2":"high (business value 100)"},
                "probabilities":{"0":0.42,"1":0.08,"2":0.5}}
        }
    })
}

#[tokio::test]
async fn maps_native_types_probabilities_and_metadata_without_inventing_confidence() {
    let request = request();
    let native = response();
    let (endpoint, captured) = server(native.clone()).await;
    let provider = AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
    let result = provider.decide(&request).await.unwrap();
    result.validate(&request).unwrap();
    assert_eq!(result.status, ResponseStatus::Complete);
    assert_eq!(
        result.answers[0].answer,
        Some(Answer::Choice("concrete_change".into()))
    );
    assert_eq!(result.answers[1].answer, Some(Answer::Boolean(true)));
    assert_eq!(
        result.answers[1].probabilities,
        Some(ProbabilityInfo::Boolean {
            true_probability: 0.5
        })
    );
    assert_eq!(result.answers[2].answer, Some(Answer::Score("high".into())));
    let Some(ProbabilityInfo::Score {
        distribution,
        expected_value,
    }) = &result.answers[2].probabilities
    else {
        panic!("native score must retain its business-level probability mapping");
    };
    assert_eq!(
        distribution
            .iter()
            .map(|entry| entry.answer.as_str())
            .collect::<Vec<_>>(),
        vec!["low", "medium", "high"]
    );
    assert!((expected_value - 50.8).abs() < 1e-9);
    assert_eq!(result.metadata.provider, "aliyun_decision");
    assert_eq!(result.metadata.model, MODEL);
    assert_eq!(
        result.metadata.request_id.as_deref(),
        Some("native-request-42")
    );
    assert_eq!(result.metadata.server_latency_ms, Some(52.9));
    assert_eq!(
        result.metadata.native_answers,
        Some(native["answers"].clone())
    );
    assert_eq!(result.metadata.usage, Some(json!({"input_tokens":125})));
    assert!(!serde_json::to_string(&result).unwrap().contains(API_KEY));
    assert!(!provider.fingerprint().contains(API_KEY));

    let (headers, body) = captured.await.unwrap();
    assert!(headers.starts_with("POST /compatible-mode/v1/systemone HTTP/1.1\r\n"));
    assert!(headers
        .to_ascii_lowercase()
        .contains("authorization: bearer synthetic-aliyun-test-key"));
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["questions"]["escalate"]["type"], "noul");
    assert_eq!(
        body["questions"]["severity"]["criteria"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert!(body.get("messages").is_none());
    assert!(body.get("tools").is_none());
}

#[tokio::test]
async fn submits_all_seven_questions_once_and_preserves_each_material_boundary() {
    let mut request = request();
    let original = request.questions[0].clone();
    request.questions = (0..7)
        .map(|index| {
            let mut question = original.clone();
            question.id = format!("question_{index}");
            question.context =
                json!({"only_for":question.id, "comments":[format!("material {index}")]});
            question
        })
        .collect();
    let native_answers: serde_json::Map<String, Value> = request
        .questions
        .iter()
        .map(|question| {
            (
                question.id.clone(),
                json!({"type":"choice", "choice":"unclear", "confidence":0.65,
            "probabilities":{"concrete_change":0.35,"unclear":0.65}}),
            )
        })
        .collect();
    let (endpoint, captured) = server(json!({"model":MODEL,"answers":native_answers})).await;
    let provider = AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
    let result = provider.decide(&request).await.unwrap();
    assert_eq!(result.answers.len(), 7);
    assert!(result
        .answers
        .iter()
        .all(|answer| answer.answer == Some(Answer::Choice("unclear".into()))));
    let (_, body) = captured.await.unwrap();
    assert_eq!(body["questions"].as_object().unwrap().len(), 7);
    assert_eq!(body["state"]["candidate_id"], request.candidate_id);
    assert_eq!(body["state"]["input_id"], request.input_id);
    for question in &request.questions {
        assert_eq!(
            body["state"]["question_materials"][&question.id],
            question.context
        );
        let instructions = body["questions"][&question.id]["instructions"]
            .as_str()
            .unwrap();
        assert!(instructions.contains(&format!("state.question_materials[{}]", json!(question.id))));
        assert!(instructions.contains(&question.prompt));
        assert!(instructions.contains(&json!(question.criteria).to_string()));
        assert_eq!(
            body["questions"][&question.id]["criteria"],
            json!({"concrete_change":"concrete_change", "unclear":"unclear"})
        );
        assert!(!instructions.contains("material 0"));
    }
}

#[tokio::test]
async fn rejects_mismatched_incomplete_or_invalid_native_answers() {
    let changes = vec![
        ("/model", json!("qwen-plus")),
        ("/answers/task_type/type", json!("score")),
        ("/answers/task_type/choice", json!("extra-option")),
        (
            "/answers/task_type/probabilities",
            json!({"concrete_change":0.8}),
        ),
        (
            "/answers/task_type/probabilities",
            json!({"concrete_change":0.8,"unclear":0.2,"other":0.0}),
        ),
        (
            "/answers/task_type/probabilities",
            json!({"concrete_change":-0.1,"unclear":1.1}),
        ),
        (
            "/answers/task_type/probabilities",
            json!({"concrete_change":0.4,"unclear":0.4}),
        ),
        (
            "/answers/task_type/probabilities/concrete_change",
            json!("94%"),
        ),
        ("/answers/task_type/confidence", Value::Null),
        ("/answers/escalate/noul", json!(1.01)),
        ("/answers/escalate/noul", json!(-0.01)),
        ("/answers/severity/score", json!(1.2)),
        (
            "/answers/severity/probabilities",
            json!({"low":0.42,"medium":0.08,"high":0.5}),
        ),
        ("/answers/severity/legend", json!({"0":"low","1":"medium"})),
        ("/answers/severity/legend/0", json!("")),
        ("/latency_ms", json!(-1.0)),
        ("/usage/input_tokens", json!(-1)),
        ("/request_id", json!("")),
    ];
    for (pointer, value) in changes {
        let mut native = response();
        *native.pointer_mut(pointer).unwrap() = value;
        let (endpoint, captured) = server(native).await;
        let provider =
            AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
        let error = provider.decide(&request()).await.unwrap_err();
        assert_eq!(error.code, "invalid_provider_response", "{pointer}");
        assert!(!error.to_string().contains(API_KEY));
        captured.await.unwrap();
    }
    for unknown_key in [false, true] {
        let mut native = response();
        let answer = native["answers"]
            .as_object_mut()
            .unwrap()
            .remove("escalate")
            .unwrap();
        if unknown_key {
            native["answers"]["unrequested"] = answer;
        }
        let (endpoint, captured) = server(native).await;
        let provider =
            AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
        assert_eq!(
            provider.decide(&request()).await.unwrap_err().code,
            "invalid_provider_response"
        );
        captured.await.unwrap();
    }
}

#[tokio::test]
async fn noul_below_half_maps_to_false_and_score_accepts_only_rounding_tolerance() {
    let mut native = response();
    native["answers"]["escalate"]["noul"] = json!(0.49);
    native["answers"]["severity"]["score"] = json!(1.08000001);
    let (endpoint, captured) = server(native).await;
    let provider = AliyunProvider::new(endpoint, API_KEY.into(), Duration::from_secs(2)).unwrap();
    let result = provider.decide(&request()).await.unwrap();
    assert_eq!(result.answers[1].answer, Some(Answer::Boolean(false)));
    captured.await.unwrap();
}

#[test]
fn rejects_chat_urls_credentials_and_insecure_remote_endpoints() {
    for endpoint in [
        "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
        "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode",
        "https://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone?api_key=secret",
        "https://secret@workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone",
        "http://workspace.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/systemone",
    ] {
        assert!(AliyunProvider::new(endpoint.into(), API_KEY.into(), Duration::from_secs(2)).is_err());
    }
    assert!(AliyunProvider::new(
        "http://127.0.0.1:1/compatible-mode/v1/systemone".into(),
        " ".into(),
        Duration::from_secs(2)
    )
    .is_err());
    assert!(AliyunProvider::new(
        "http://127.0.0.1:1/compatible-mode/v1/systemone".into(),
        API_KEY.into(),
        Duration::ZERO
    )
    .is_err());
}

#[tokio::test]
async fn rejects_unsupported_score_rubric_before_network_admission() {
    let provider = AliyunProvider::new(
        "http://127.0.0.1:1/compatible-mode/v1/systemone".into(),
        API_KEY.into(),
        Duration::from_secs(2),
    )
    .unwrap();
    let mut request = request();
    request.questions[2].kind = QuestionKind::Score {
        levels: (0..11)
            .map(|index| ScoreLevel {
                id: index.to_string(),
                value: index as f64,
            })
            .collect(),
    };
    assert_eq!(
        provider.decide(&request).await.unwrap_err().code,
        "invalid_contract"
    );
}
