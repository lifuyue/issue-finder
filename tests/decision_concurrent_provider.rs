#![cfg(unix)]

use std::collections::BTreeSet;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use issue_finder::decision::codex::CodexProvider;
use issue_finder::decision::contract::*;
use serde_json::{json, Value};
use tempfile::TempDir;

// Concurrent batches use request-count barriers; the close test explicitly
// releases its server through a file gate. A serialized provider cannot get
// past the four-request barrier.
const SERVER: &str = r#"
import json, os, sys, time

if '--version' in sys.argv:
    print('codex-cli 0.159.3')
    sys.exit(0)
if '--help' in sys.argv:
    sys.exit(0)

base = os.path.abspath(sys.argv[0])
mode = MODE
with open(base + '.starts', 'a') as log:
    log.write(str(os.getpid()) + '\n')
threads, starts, turns, attempts = {}, [], [], {}
sequence = 0
released = False

def record(direction, message):
    with open(base + '.trace', 'a') as log:
        log.write(json.dumps({'direction': direction, 'time': time.monotonic(), 'message': message}) + '\n')

def send(message):
    record('out', message)
    print(json.dumps(message), flush=True)

def result(message, value):
    send({'id': message['id'], 'result': value})

def thread_result(message, thread):
    result(message, {'model': 'gpt-6-luna', 'reasoningEffort': 'none', 'thread': {'id': thread}})

def event(method, thread, turn_id, **values):
    send({'method': method, 'params': dict(threadId=thread, turnId=turn_id, **values)})

def answer(request):
    return {'candidate_id': request['candidate_id'], 'input_id': request['input_id'], 'answers': [
        {'question_id': request['questions'][0]['id'], 'status': 'answered',
         'answer': {'type': 'choice', 'value': request['candidate_id']}}]}

def final(thread, turn, request):
    event('item/completed', thread, turn, item={'id': 'item-' + thread,
        'type': 'agentMessage', 'phase': 'final_answer', 'text': json.dumps(answer(request))})

def usage(thread, turn, request):
    number = int(request['input_id'].split('-')[-1])
    event('thread/tokenUsage/updated', thread, turn,
        tokenUsage={'last': {'inputTokens': 100 + number, 'outputTokens': number}})

def complete(thread, turn):
    event('turn/completed', thread, turn, turn={'id': turn, 'status': 'completed'})

def success(thread, turn, request):
    final(thread, turn, request)
    usage(thread, turn, request)
    complete(thread, turn)

for line in sys.stdin:
    message = json.loads(line)
    record('in', message)
    method = message.get('method')
    params = message.get('params', {})
    if method == 'initialize':
        result(message, {})
    elif method == 'thread/start':
        sequence += 1
        thread = 'thread-' + str(sequence)
        threads[thread] = {}
        if mode == 'late_thread_response' and not released:
            starts.append((message, thread))
            if len(starts) == 2:
                released = True
                for pending, pending_thread in reversed(starts):
                    thread_result(pending, pending_thread)
        elif mode in ('interleaved', 'isolation', 'exit', 'close_wait') and not released:
            starts.append((message, thread))
            if len(starts) == 4:
                for pending, pending_thread in reversed(starts):
                    thread_result(pending, pending_thread)
        else:
            thread_result(message, thread)
    elif method == 'turn/start':
        thread = params['threadId']
        request = json.loads(params['input'][0]['text'])
        candidate = request['candidate_id']
        attempts[candidate] = attempts.get(candidate, 0) + 1
        turn = 'turn-' + thread + '-' + str(attempts[candidate])
        threads[thread] = {'request': request, 'turn': turn}
        if mode == 'flood' and not released:
            turns.append((message, thread, turn, request))
            if len(turns) != 2:
                continue
            released = True
            flooded = next(info for info in turns if info[3]['candidate_id'] == 'flooded')
            healthy = next(info for info in turns if info[3]['candidate_id'] == 'healthy')
            pending, t, v, _ = flooded
            for number in range(300):
                event('item/started', t, v, item={'id': 'reasoning-' + str(number), 'type': 'reasoning'})
                if number == 150:
                    peer_rpc, peer_thread, peer_turn, peer_request = healthy
                    success(peer_thread, peer_turn, peer_request)
                    result(peer_rpc, {'turn': {'id': peer_turn}})
            result(pending, {'turn': {'id': v}})
        elif mode == 'lost_turn_response' and candidate == 'lost-response':
            # The accepted turn is observable even though its RPC response is lost.
            send({'method': 'turn/started', 'params': {'threadId': thread,
                'turn': {'id': turn, 'status': 'inProgress'}}})
            event('item/started', thread, turn, item={'id': 'reasoning', 'type': 'reasoning'})
        elif mode in ('interleaved', 'isolation', 'exit', 'close_wait') and not released:
            turns.append((message, thread, turn, request))
            if len(turns) != 4:
                continue
            released = True
            if mode == 'exit':
                os._exit(7)
            if mode == 'close_wait':
                while not os.path.exists(base + '.release'):
                    time.sleep(0.005)
            if mode == 'interleaved':
                for _, t, v, r in turns:
                    final(t, v, r)
                for _, t, v, r in reversed(turns):
                    usage(t, v, r)
                for _, t, v, _ in turns[1:] + turns[:1]:
                    complete(t, v)
                for pending, _, v, _ in reversed(turns):
                    result(pending, {'turn': {'id': v}})
            elif mode == 'close_wait':
                for pending, t, v, r in reversed(turns):
                    result(pending, {'turn': {'id': v}})
                    success(t, v, r)
            else:
                for pending, _, v, _ in reversed(turns):
                    result(pending, {'turn': {'id': v}})
                for _, t, v, r in turns:
                    if r['candidate_id'] == 'approval':
                        send({'id': 9000, 'method': 'item/commandExecution/requestApproval',
                            'params': {'threadId': t, 'turnId': v, 'itemId': 'approval-item',
                                       'command': 'printf unsafe'}})
                    elif r['candidate_id'] == 'healthy':
                        success(t, v, r)
        elif mode.startswith('retry'):
            if mode == 'retry_once' and attempts[candidate] > 1:
                result(message, {'turn': {'id': turn}})
                success(thread, turn, request)
            else:
                delay = 1500 if mode == 'retry_deadline' else 150
                send({'id': message['id'], 'error': {'code': -32001,
                    'message': 'Server overloaded; request was not accepted',
                    'data': {'retry_after_ms': delay}}})
        elif mode == 'authentication':
            send({'id': message['id'], 'error': {'code': -32001,
                'message': 'Unauthorized: access token expired'}})
        elif mode == 'unknown_acceptance':
            send({'id': message['id'], 'error': {'code': -32603,
                'message': 'Server overloaded; acceptance is unknown'}})
        elif mode == 'accepted_without_turn_id':
            result(message, {'turn': {'status': 'inProgress'}})
        elif mode == 'accepted_failure':
            result(message, {'turn': {'id': turn}})
            event('turn/completed', thread, turn, turn={'id': turn, 'status': 'failed',
                'error': {'code': -32001, 'message': 'Server overloaded; retry after 0.1 seconds'}})
        else:
            result(message, {'turn': {'id': turn}})
            success(thread, turn, request)
    elif method == 'thread/read':
        info = threads.get(params['threadId'], {})
        known = info.get('turn')
        result(message, {'thread': {'id': params['threadId'], 'turns':
            [{'id': known, 'status': 'inProgress'}] if known else []}})
    elif method == 'turn/interrupt':
        result(message, {})
    elif method == 'thread/unsubscribe':
        thread = params['threadId']
        info = threads.get(thread, {})
        # Delayed traffic from a cancelled/expired turn must not poison its peers.
        if info.get('request', {}).get('candidate_id') in ('cancelled', 'stalled'):
            success(thread, info['turn'], info['request'])
        result(message, {})
"#;

struct FakeCodex {
    _directory: TempDir,
    binary: PathBuf,
}

impl FakeCodex {
    fn new(mode: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("codex");
        let python = std::process::Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .expect("these offline app-server tests require python3");
        assert!(python.status.success());
        let source = format!(
            "#!{}\n{}",
            String::from_utf8(python.stdout).unwrap().trim(),
            SERVER.replace("MODE", &serde_json::to_string(mode).unwrap())
        );
        std::fs::write(&binary, source).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _directory: directory,
            binary,
        }
    }

    fn provider(&self, timeout: Duration) -> Arc<CodexProvider> {
        Arc::new(
            CodexProvider::new(Some(self.binary.to_string_lossy().into_owned()), timeout).unwrap(),
        )
    }

    fn side_file(&self, suffix: &str) -> PathBuf {
        PathBuf::from(format!("{}.{}", self.binary.display(), suffix))
    }

    fn trace(&self) -> Vec<Value> {
        std::fs::read_to_string(self.side_file("trace"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn requests(&self, method: &str) -> Vec<Value> {
        self.trace()
            .into_iter()
            .filter(|entry| entry["direction"] == "in" && entry["message"]["method"] == method)
            .collect()
    }

    async fn wait_for_turns(&self, count: usize) {
        self.wait_for_requests("turn/start", count).await;
    }

    async fn wait_for_requests(&self, method: &str, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.requests(method).len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("expected concurrent requests did not reach the app-server");
    }

    fn assert_one_process(&self) {
        assert_eq!(
            std::fs::read_to_string(self.side_file("starts"))
                .unwrap()
                .lines()
                .count(),
            1,
            "independent classifications must share one app-server"
        );
    }

    fn assert_process_stopped(&self) {
        for pid in std::fs::read_to_string(self.side_file("starts"))
            .unwrap()
            .lines()
        {
            let pid: i32 = pid.parse().unwrap();
            assert!(
                unsafe { libc::kill(pid, 0) } != 0,
                "child {pid} still exists"
            );
        }
    }
}

fn request(candidate: &str, number: usize) -> DecisionRequest {
    DecisionRequest {
        candidate_id: candidate.into(),
        input_id: format!("input-{number}"),
        questions: vec![Question {
            id: format!("question-{number}"),
            prompt: "Which supplied issue does this evidence identify?".into(),
            criteria: vec!["Choose the identifier present in the evidence".into()],
            context: json!({"issue":candidate}),
            kind: QuestionKind::Choice {
                options: vec![candidate.into(), "other".into()],
            },
        }],
    }
}

fn spawn_decision(
    provider: &Arc<CodexProvider>,
    candidate: &str,
    number: usize,
) -> tokio::task::JoinHandle<Result<DecisionResponse, ProviderError>> {
    let provider = Arc::clone(provider);
    let request = request(candidate, number);
    tokio::spawn(async move { provider.decide(&request).await })
}

fn assert_answer(response: &DecisionResponse, candidate: &str, number: usize) {
    assert_eq!(response.status, ResponseStatus::Complete);
    assert_eq!(response.candidate_id, candidate);
    assert_eq!(response.input_id, format!("input-{number}"));
    assert_eq!(
        response.answers[0].question_id,
        format!("question-{number}")
    );
    assert_eq!(
        response.answers[0].answer,
        Some(Answer::Choice(candidate.into()))
    );
    assert!(response.answers[0].probabilities.is_none());
    assert_eq!(
        response.metadata.usage,
        Some(json!({"inputTokens":100 + number,"outputTokens":number}))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn four_threads_route_interleaved_notifications_before_reordered_rpc_responses() {
    let fake = FakeCodex::new("interleaved");
    let provider = fake.provider(Duration::from_secs(4));
    let jobs: Vec<_> = (0..4)
        .map(|number| spawn_decision(&provider, &format!("issue-{number}"), number))
        .collect();
    for (number, job) in jobs.into_iter().enumerate() {
        let response = job.await.unwrap().unwrap();
        assert_answer(&response, &format!("issue-{number}"), number);
    }
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();

    let starts = fake.requests("thread/start");
    assert_eq!(starts.len(), 4);
    for entry in &starts {
        let params = &entry["message"]["params"];
        assert_eq!(params["ephemeral"], true);
        assert_eq!(params["allowProviderModelFallback"], false);
        assert_eq!(params["dynamicTools"], json!([]));
        assert_eq!(params["approvalPolicy"], "never");
    }
    let turns = fake.requests("turn/start");
    let threads: BTreeSet<_> = turns
        .iter()
        .map(|entry| entry["message"]["params"]["threadId"].as_str().unwrap())
        .collect();
    assert_eq!(threads.len(), 4);
    assert_eq!(fake.requests("thread/unsubscribe").len(), 4);
    let trace = fake.trace();
    let turn_ids: BTreeSet<_> = turns
        .iter()
        .map(|entry| entry["message"]["id"].clone().to_string())
        .collect();
    let first_turn_reply = trace
        .iter()
        .position(|entry| {
            entry["direction"] == "out" && turn_ids.contains(&entry["message"]["id"].to_string())
        })
        .unwrap();
    assert_eq!(
        trace[..first_turn_reply]
            .iter()
            .filter(|entry| {
                entry["direction"] == "out" && entry["message"]["method"] == "turn/completed"
            })
            .count(),
        4
    );
    let thread_ids: Vec<_> = starts
        .iter()
        .map(|entry| entry["message"]["id"].clone())
        .collect();
    let replies: Vec<_> = trace
        .iter()
        .filter(|entry| entry["direction"] == "out" && thread_ids.contains(&entry["message"]["id"]))
        .map(|entry| entry["message"]["id"].clone())
        .collect();
    assert_eq!(replies, thread_ids.into_iter().rev().collect::<Vec<_>>());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approval_failure_timeout_and_cancellation_leave_peer_threads_and_process_usable() {
    let fake = FakeCodex::new("isolation");
    let provider = fake.provider(Duration::from_millis(1000));
    let rejected = spawn_decision(&provider, "approval", 0);
    let cancelled = spawn_decision(&provider, "cancelled", 1);
    let stalled = spawn_decision(&provider, "stalled", 2);
    let healthy = spawn_decision(&provider, "healthy", 3);
    fake.wait_for_turns(4).await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    assert_eq!(
        rejected.await.unwrap().unwrap_err().code,
        "unexpected_tool_request"
    );
    assert_answer(&healthy.await.unwrap().unwrap(), "healthy", 3);
    assert_eq!(stalled.await.unwrap().unwrap_err().code, "timeout");
    assert_answer(
        &provider.decide(&request("after-cleanup", 4)).await.unwrap(),
        "after-cleanup",
        4,
    );
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    let trace = fake.trace();
    assert!(trace.iter().any(|entry| entry["direction"] == "in"
        && entry["message"]["id"] == 9000
        && entry["message"]["error"]["code"] == -32601));
    let unsubscribed: BTreeSet<_> = fake
        .requests("thread/unsubscribe")
        .into_iter()
        .map(|entry| entry["message"]["params"]["threadId"].clone().to_string())
        .collect();
    assert_eq!(
        unsubscribed.len(),
        5,
        "every completed, rejected, expired or cancelled thread must be cleaned up"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_process_exit_finishes_all_four_waiters_without_retrying_turns() {
    let fake = FakeCodex::new("exit");
    let provider = fake.provider(Duration::from_secs(4));
    let jobs: Vec<_> = (0..4)
        .map(|number| spawn_decision(&provider, &format!("issue-{number}"), number))
        .collect();
    let start = tokio::time::Instant::now();
    for job in jobs {
        let error = job.await.unwrap().unwrap_err();
        assert!(
            matches!(error.code.as_str(), "transport_closed" | "transport_error"),
            "{error}"
        );
    }
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "shared exit must wake every waiter before its deadline"
    );
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    assert_eq!(fake.requests("turn/start").len(), 4);
}

#[tokio::test]
async fn explicit_unaccepted_overload_retries_once_after_the_server_delay() {
    let fake = FakeCodex::new("retry_once");
    let provider = fake.provider(Duration::from_secs(3));
    assert_answer(
        &provider.decide(&request("overloaded", 0)).await.unwrap(),
        "overloaded",
        0,
    );
    provider.close().await;
    let attempts = fake.requests("turn/start");
    assert_eq!(attempts.len(), 2);
    let delay = attempts[1]["time"].as_f64().unwrap() - attempts[0]["time"].as_f64().unwrap();
    assert!(delay >= 0.14, "retry_after_ms=150 was ignored: {delay}s");
    fake.assert_one_process();
    fake.assert_process_stopped();
}

#[tokio::test]
async fn repeated_safe_overload_stops_after_one_retry() {
    let fake = FakeCodex::new("retry_always");
    let provider = fake.provider(Duration::from_secs(3));
    assert!(provider.decide(&request("overloaded", 0)).await.is_err());
    provider.close().await;
    assert_eq!(fake.requests("turn/start").len(), 2);
    fake.assert_one_process();
    fake.assert_process_stopped();
}

#[tokio::test]
async fn retry_delay_cannot_extend_the_classification_deadline() {
    let fake = FakeCodex::new("retry_deadline");
    let provider = fake.provider(Duration::from_millis(300));
    assert_eq!(
        provider
            .decide(&request("overloaded", 0))
            .await
            .unwrap_err()
            .code,
        "timeout"
    );
    provider.close().await;
    assert_eq!(fake.requests("turn/start").len(), 1);
    fake.assert_process_stopped();
}

#[tokio::test]
async fn authentication_unknown_acceptance_and_accepted_turn_failure_never_retry() {
    for mode in [
        "authentication",
        "unknown_acceptance",
        "accepted_without_turn_id",
        "accepted_failure",
    ] {
        let fake = FakeCodex::new(mode);
        let provider = fake.provider(Duration::from_secs(3));
        let error = provider.decide(&request("issue", 0)).await.unwrap_err();
        if mode == "authentication" {
            assert_eq!(error.code, "authentication_failed");
        }
        provider.close().await;
        assert_eq!(
            fake.requests("turn/start").len(),
            1,
            "{mode} must not retry"
        );
        fake.assert_one_process();
        fake.assert_process_stopped();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overflowing_one_thread_event_buffer_preserves_healthy_peer_notifications() {
    let fake = FakeCodex::new("flood");
    let provider = fake.provider(Duration::from_secs(3));
    let flooded = spawn_decision(&provider, "flooded", 0);
    let healthy = spawn_decision(&provider, "healthy", 1);
    let error = flooded.await.unwrap().unwrap_err();
    assert_eq!(error.code, "invalid_protocol");
    assert!(error.message.contains("bound"), "{error}");
    assert_answer(&healthy.await.unwrap().unwrap(), "healthy", 1);
    assert_answer(
        &provider.decide(&request("after-flood", 2)).await.unwrap(),
        "after-flood",
        2,
    );
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    assert_eq!(fake.requests("turn/start").len(), 3);
    assert_eq!(fake.requests("thread/unsubscribe").len(), 3);
    let interrupts = fake.requests("turn/interrupt");
    assert_eq!(interrupts.len(), 1);
    let starts = fake.requests("turn/start");
    let flooded_thread = &starts
        .iter()
        .find(|entry| {
            let prompt = entry["message"]["params"]["input"][0]["text"]
                .as_str()
                .unwrap();
            serde_json::from_str::<Value>(prompt).unwrap()["candidate_id"] == "flooded"
        })
        .unwrap()["message"]["params"]["threadId"];
    assert_eq!(
        &interrupts[0]["message"]["params"]["threadId"],
        flooded_thread
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_turn_start_response_times_out_without_replay_and_cleans_up_observed_turn() {
    let fake = FakeCodex::new("lost_turn_response");
    let provider = fake.provider(Duration::from_millis(350));
    let lost = spawn_decision(&provider, "lost-response", 0);
    let healthy = spawn_decision(&provider, "healthy", 1);
    assert_answer(&healthy.await.unwrap().unwrap(), "healthy", 1);
    assert_eq!(lost.await.unwrap().unwrap_err().code, "timeout");
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    assert_eq!(
        fake.requests("turn/start").len(),
        2,
        "a missing response cannot safely be replayed"
    );
    let trace = fake.trace();
    let started = trace
        .iter()
        .find(|entry| entry["direction"] == "out" && entry["message"]["method"] == "turn/started")
        .unwrap();
    let thread_id = &started["message"]["params"]["threadId"];
    let turn_id = &started["message"]["params"]["turn"]["id"];
    let interrupts = fake.requests("turn/interrupt");
    assert_eq!(interrupts.len(), 1);
    assert_eq!(&interrupts[0]["message"]["params"]["threadId"], thread_id);
    assert_eq!(&interrupts[0]["message"]["params"]["turnId"], turn_id);
    assert!(fake
        .requests("thread/unsubscribe")
        .iter()
        .any(|entry| { &entry["message"]["params"]["threadId"] == thread_id }));
    assert_eq!(fake.requests("thread/unsubscribe").len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_thread_start_with_late_response_is_unsubscribed_without_starting_a_turn() {
    let fake = FakeCodex::new("late_thread_response");
    let provider = fake.provider(Duration::from_secs(3));
    let cancelled = spawn_decision(&provider, "cancelled-before-thread-response", 0);
    fake.wait_for_requests("thread/start", 1).await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    // The healthy request releases the fake's delayed response to the cancelled RPC.
    let healthy = spawn_decision(&provider, "healthy", 1);
    assert_answer(&healthy.await.unwrap().unwrap(), "healthy", 1);
    provider.close().await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    assert_eq!(fake.requests("thread/start").len(), 2);
    assert_eq!(fake.requests("turn/start").len(), 1);
    let unsubscribed: BTreeSet<_> = fake
        .requests("thread/unsubscribe")
        .into_iter()
        .map(|entry| entry["message"]["params"]["threadId"].clone())
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        unsubscribed,
        BTreeSet::from(["thread-1".into(), "thread-2".into()])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_waits_for_four_active_decisions_and_cleanup_while_rejecting_new_work() {
    let fake = FakeCodex::new("close_wait");
    let provider = fake.provider(Duration::from_secs(3));
    let jobs: Vec<_> = (0..4)
        .map(|number| spawn_decision(&provider, &format!("issue-{number}"), number))
        .collect();
    fake.wait_for_turns(4).await;

    // Polling once enters the closing state synchronously, then waits for leases.
    let close = provider.close();
    tokio::pin!(close);
    assert!(futures::poll!(&mut close).is_pending());
    assert_eq!(
        provider
            .decide(&request("after-close", 4))
            .await
            .unwrap_err()
            .code,
        "transport_closed"
    );
    assert!(futures::poll!(&mut close).is_pending());
    std::fs::write(fake.side_file("release"), b"release pending turns").unwrap();
    for (number, job) in jobs.into_iter().enumerate() {
        assert_answer(
            &job.await.unwrap().unwrap(),
            &format!("issue-{number}"),
            number,
        );
    }
    close.await;
    fake.assert_one_process();
    fake.assert_process_stopped();
    assert_eq!(fake.requests("thread/start").len(), 4);
    assert_eq!(fake.requests("turn/start").len(), 4);
    assert_eq!(fake.requests("thread/unsubscribe").len(), 4);
}
