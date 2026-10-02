#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use issue_finder::decision::codex::CodexProvider;
use issue_finder::decision::contract::*;
use serde_json::{json, Value};
use tempfile::TempDir;

fn request() -> DecisionRequest {
    DecisionRequest {
        candidate_id: "repo#1".into(),
        input_id: "input-v1".into(),
        questions: vec![Question {
            id: "kind".into(),
            prompt: "What kind of task is proposed?".into(),
            criteria: vec!["Choose concrete only when a specific change is requested".into()],
            context: json!({"title":"Correct a broken documentation link"}),
            kind: QuestionKind::Choice {
                options: vec!["concrete".into(), "unclear".into()],
            },
        }],
    }
}

fn valid_answer() -> Value {
    json!({"candidate_id":"repo#1","input_id":"input-v1","answers":[{"question_id":"kind","status":"answered","answer":{"type":"choice","value":"concrete"}}]})
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

struct FakeCodex {
    _directory: TempDir,
    binary: PathBuf,
}

impl FakeCodex {
    fn new(mode: &str, answer: Value) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("codex");
        let text = if mode == "output_json" {
            "this is not JSON".to_string()
        } else {
            answer.to_string()
        };
        let output = json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1","item":{"type":"agentMessage","phase":"final_answer","text":text}}}).to_string();
        let source = format!(
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo 'codex-cli 0.159.3'; exit 0; fi
if [ "$2" = "--help" ]; then exit 0; fi
mode={mode}
echo start >> "$0.starts"
echo $$ > "$0.pid"
while IFS= read -r line; do
  echo "$line" >> "$0.requests"
  id=$(echo "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) echo '{{"id":'"$id"',"result":{{}}}}' ;;
    *'"method":"thread/start"'*)
      model='gpt-6-luna'; effort='none'
      if [ "$mode" = model ]; then model=other; fi
      if [ "$mode" = effort ]; then effort=low; fi
      echo '{{"id":'"$id"',"result":{{"model":"'"$model"'","reasoningEffort":"'"$effort"'","thread":{{"id":"thread-1"}}}}}}' ;;
    *'"method":"turn/start"'*)
      echo '{{"id":'"$id"',"result":{{"turn":{{"id":"turn-1"}}}}}}'
      if [ "$mode" = timeout ]; then while IFS= read -r ignored; do :; done; exit 0; fi
      if [ "$mode" = approval ]; then echo '{{"id":999,"method":"item/commandExecution/requestApproval","params":{{}}}}'; continue; fi
      if [ "$mode" = tool ]; then echo '{{"method":"item/started","params":{{"threadId":"thread-1","turnId":"turn-1","item":{{"type":"commandExecution"}}}}}}'; continue; fi
      if [ "$mode" = auth ]; then echo '{{"method":"turn/completed","params":{{"threadId":"thread-1","turnId":"turn-1","turn":{{"status":"failed","error":{{"message":"Unauthorized: access token expired"}}}}}}}}'; continue; fi
      if [ "$mode" = protocol ]; then echo invalid-json; continue; fi
      printf '%s\n' {output}
      echo '{{"method":"thread/tokenUsage/updated","params":{{"threadId":"thread-1","tokenUsage":{{"last":{{"inputTokens":100,"outputTokens":10}}}}}}}}'
      echo '{{"method":"turn/completed","params":{{"threadId":"thread-1","turnId":"turn-1","turn":{{"id":"turn-1","status":"completed"}}}}}}' ;;
    *'"method":"thread/unsubscribe"'*) echo '{{"id":'"$id"',"result":{{}}}}' ;;
  esac
done
"#,
            mode = shell_quote(mode),
            output = shell_quote(&output),
        );
        std::fs::write(&binary, source).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _directory: directory,
            binary,
        }
    }

    fn provider(&self, timeout: Duration) -> CodexProvider {
        CodexProvider::new(Some(self.binary.to_string_lossy().into()), timeout).unwrap()
    }

    fn side_file(&self, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}.{}", self.binary.display(), suffix))
    }

    fn process_stopped(&self) -> bool {
        let pid: i32 = std::fs::read_to_string(self.side_file("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        unsafe { libc::kill(pid, 0) != 0 }
    }
}

#[tokio::test]
async fn persistent_adapter_uses_fixed_model_and_isolated_tool_free_threads() {
    let fake = FakeCodex::new("success", valid_answer());
    let provider = fake.provider(Duration::from_secs(3));
    for _ in 0..2 {
        let response = provider.decide(&request()).await.unwrap();
        assert_eq!(response.status, ResponseStatus::Complete);
        assert_eq!(
            response.answers[0].answer,
            Some(Answer::Choice("concrete".into()))
        );
        assert!(response.answers[0].probabilities.is_none());
        assert_eq!(response.metadata.model, "gpt-6-luna");
        assert_eq!(response.metadata.reasoning_effort, "none");
    }
    assert_eq!(
        std::fs::read_to_string(fake.side_file("starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let requests: Vec<Value> = std::fs::read_to_string(fake.side_file("requests"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let threads: Vec<_> = requests
        .iter()
        .filter(|value| value["method"] == "thread/start")
        .collect();
    assert_eq!(threads.len(), 2);
    for thread in threads {
        let params = &thread["params"];
        assert_eq!(params["ephemeral"], true);
        assert_eq!(params["allowProviderModelFallback"], false);
        assert_eq!(params["environments"], json!([]));
        assert_eq!(params["dynamicTools"], json!([]));
        assert_eq!(params["selectedCapabilityRoots"], json!([]));
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["config"]["features.shell_tool"], false);
        assert!(params["cwd"]
            .as_str()
            .unwrap()
            .contains("issue-finder-decision-"));
    }
    assert!(provider.fingerprint().contains("version=codex-cli 0.159.3"));
    provider.close().await;
    assert!(fake.process_stopped());
}

#[tokio::test]
async fn adapter_rejects_model_effort_tools_authentication_and_protocol_errors() {
    for (mode, expected) in [
        ("model", "model_mismatch"),
        ("effort", "reasoning_mismatch"),
        ("tool", "unexpected_tool_use"),
        ("approval", "unexpected_tool_request"),
        ("auth", "authentication_failed"),
        ("protocol", "invalid_protocol"),
        ("output_json", "invalid_output"),
    ] {
        let fake = FakeCodex::new(mode, valid_answer());
        let provider = fake.provider(Duration::from_secs(2));
        assert_eq!(
            provider.decide(&request()).await.unwrap_err().code,
            expected,
            "mode {mode}"
        );
        if mode != "protocol" {
            assert!(
                !fake.process_stopped(),
                "individual error must not stop shared process: {mode}"
            );
        }
        provider.close().await;
        assert!(fake.process_stopped(), "close should kill and reap: {mode}");
    }
}

#[tokio::test]
async fn invalid_outputs_never_become_valid_business_answers() {
    let mut cases = Vec::new();
    let mut wrong_candidate = valid_answer();
    wrong_candidate["candidate_id"] = json!("repo#2");
    cases.push(wrong_candidate);
    let mut wrong_input = valid_answer();
    wrong_input["input_id"] = json!("input-v2");
    cases.push(wrong_input);
    let mut wrong_question = valid_answer();
    wrong_question["answers"][0]["question_id"] = json!("other");
    cases.push(wrong_question);
    let mut wrong_value = valid_answer();
    wrong_value["answers"][0]["answer"]["value"] = json!("invented");
    cases.push(wrong_value);
    let mut missing = valid_answer();
    missing["answers"] = json!([]);
    cases.push(missing);
    let mut duplicate = valid_answer();
    duplicate["answers"] = json!([
        duplicate["answers"][0].clone(),
        duplicate["answers"][0].clone()
    ]);
    cases.push(duplicate);
    let mut probability = valid_answer();
    probability["answers"][0]["probabilities"] = json!({"true_probability":0.99});
    cases.push(probability);
    let mut contradictory = valid_answer();
    contradictory["answers"][0]["status"] = json!("unable_to_answer");
    cases.push(contradictory);
    let mut missing_answer = valid_answer();
    missing_answer["answers"][0]["status"] = json!("unable_to_answer");
    missing_answer["answers"][0]
        .as_object_mut()
        .unwrap()
        .remove("answer");
    cases.push(missing_answer);
    for answer in cases {
        let fake = FakeCodex::new("success", answer);
        let provider = fake.provider(Duration::from_secs(2));
        assert_eq!(
            provider.decide(&request()).await.unwrap_err().code,
            "invalid_output"
        );
        assert!(!fake.process_stopped());
        provider.close().await;
        assert!(fake.process_stopped());
    }
}

#[tokio::test]
async fn deadline_isolates_stalled_turn_and_preserves_shared_process() {
    let fake = FakeCodex::new("timeout", valid_answer());
    let provider = fake.provider(Duration::from_millis(300));
    for _ in 0..2 {
        assert_eq!(
            provider.decide(&request()).await.unwrap_err().code,
            "timeout"
        );
        assert!(!fake.process_stopped());
    }
    assert_eq!(
        std::fs::read_to_string(fake.side_file("starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    provider.close().await;
    assert!(fake.process_stopped());
}

#[tokio::test]
async fn inability_to_answer_is_partial_and_carries_no_negative_answer() {
    let mut answer = valid_answer();
    answer["answers"][0]["status"] = json!("unable_to_answer");
    answer["answers"][0]["answer"] = Value::Null;
    let fake = FakeCodex::new("success", answer);
    let provider = fake.provider(Duration::from_secs(2));
    let response = provider.decide(&request()).await.unwrap();
    assert_eq!(response.status, ResponseStatus::Partial);
    assert_eq!(response.answers[0].status, AnswerStatus::UnableToAnswer);
    assert!(response.answers[0].answer.is_none());
    assert!(response.answers[0].probabilities.is_none());
    provider.close().await;
}

#[test]
fn unified_contract_keeps_scores_and_native_expectations_distinct() {
    let request = DecisionRequest {
        candidate_id: "repo#1".into(),
        input_id: "input-v1".into(),
        questions: vec![Question {
            id: "scope".into(),
            prompt: "How bounded?".into(),
            criteria: vec!["Use the supplied ordered levels".into()],
            context: json!({}),
            kind: QuestionKind::Score {
                levels: vec![
                    ScoreLevel {
                        id: "small".into(),
                        value: 1.0,
                    },
                    ScoreLevel {
                        id: "large".into(),
                        value: 3.0,
                    },
                ],
            },
        }],
    };
    let mut response = DecisionResponse {
        candidate_id: "repo#1".into(),
        input_id: "input-v1".into(),
        status: ResponseStatus::Complete,
        answers: vec![QuestionResponse {
            question_id: "scope".into(),
            status: AnswerStatus::Answered,
            answer: Some(Answer::Score("small".into())),
            probabilities: Some(ProbabilityInfo::Score {
                distribution: vec![
                    AnswerProbability {
                        answer: "small".into(),
                        probability: 0.75,
                    },
                    AnswerProbability {
                        answer: "large".into(),
                        probability: 0.25,
                    },
                ],
                expected_value: 1.5,
            }),
        }],
        metadata: ProviderMetadata::default(),
    };
    response.validate(&request).unwrap();
    if let Some(ProbabilityInfo::Score { expected_value, .. }) =
        &mut response.answers[0].probabilities
    {
        *expected_value = 1.0;
    }
    assert!(response.validate(&request).is_err());
    response.answers[0].probabilities = None;
    response.answers[0].status = AnswerStatus::UnableToAnswer;
    response.answers[0].answer = None;
    response.status = ResponseStatus::Partial;
    response.validate(&request).unwrap();
}
