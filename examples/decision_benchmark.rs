//! Explicit manual native smoke/performance runner; never part of `cargo test`.
#[path = "support/decision_fixtures.rs"]
mod fixtures;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, ValueEnum};
use futures::{stream, StreamExt};
use issue_finder::config::{DecisionConfig, DecisionProvider};
use issue_finder::decision::contract::{AnswerStatus, DecisionRequest, Provider, ResponseStatus};
use issue_finder::decision::provider::ConfiguredProvider;
use issue_finder::decision::questions::SemanticAnswers;
use issue_finder::decision::request_for;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    /// One request per selected frozen snapshot, at concurrency four.
    Smoke,
    /// 24 logical calls per run, with concurrency [4, 8, 8, 4] (96 maximum).
    Concurrency,
    /// Export the exact 24-call benchmark workload without contacting a provider.
    Workload,
}

#[derive(Clone, Copy, ValueEnum)]
enum NativeProvider {
    AliyunDecision,
    CloudflareClefFlash,
}

impl NativeProvider {
    fn config(self) -> (DecisionProvider, &'static str, &'static str) {
        match self {
            Self::AliyunDecision => (
                DecisionProvider::AliyunDecision,
                "aliyun_decision",
                "decision-model-preview",
            ),
            Self::CloudflareClefFlash => (
                DecisionProvider::CloudflareClefFlash,
                "cloudflare_clef_flash",
                "clef-flash",
            ),
        }
    }
}

#[derive(Parser)]
struct Args {
    #[arg(value_enum)]
    mode: Mode,
    #[arg(long, value_enum, default_value = "aliyun-decision")]
    provider: NativeProvider,
    /// Absolute writable JSON destination, checked before any provider calls.
    #[arg(long)]
    report: PathBuf,
    /// Explicitly permit real native provider requests and their potential cost.
    #[arg(long)]
    live: bool,
    /// Comma-separated frozen real snapshot IDs; smoke mode only.
    #[arg(long)]
    ids: Option<String>,
}

struct Input {
    sample: Value,
    request: DecisionRequest,
}

fn workload(args: &Args) -> Result<Vec<Input>> {
    let fixture = fixtures::dataset();
    let samples: Vec<_> = fixture["samples"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sample| sample["provenance"]["kind"] == "real_github_snapshot")
        .collect();
    ensure!(samples.len() == 5, "expected five frozen real snapshots");
    let ids: Vec<_> = args
        .ids
        .as_deref()
        .map(|ids| ids.split(',').map(str::trim).collect())
        .unwrap_or_default();
    ensure!(
        ids.is_empty() || args.mode == Mode::Smoke,
        "--ids is supported only in smoke mode"
    );
    ensure!(
        ids.iter()
            .all(|id| samples.iter().any(|sample| sample["id"] == *id)),
        "--ids must contain only frozen real snapshot IDs"
    );
    let selected: Vec<_> = samples
        .into_iter()
        .filter(|sample| ids.is_empty() || ids.contains(&sample["id"].as_str().unwrap()))
        .collect();
    let count = if args.mode == Mode::Smoke {
        selected.len()
    } else {
        24
    };
    (0..count)
        .map(|index| {
            let sample = selected[index % selected.len()];
            let request = request_for(&fixtures::inputs(sample).1, &fixtures::profile());
            request.validate().context("invalid frozen request")?;
            ensure!(
                request.questions.len() == 7,
                "expected seven current semantic questions"
            );
            Ok(Input {
                sample: sample.clone(),
                request,
            })
        })
        .collect()
}

fn manifest(workload: &[Input]) -> Value {
    let serialized = serde_json::to_vec(
        &workload
            .iter()
            .map(|input| &input.request)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let calls: Vec<_> = workload
        .iter()
        .enumerate()
        .map(|(index, input)| {
            let bytes = serde_json::to_vec(&input.request).unwrap();
            json!({"sample_index":index,"sample_id":input.sample["id"],
            "source_url":input.sample["issue"]["url"],"candidate":input.request.candidate_id,
            "input_id":input.request.input_id,"serialized_request_bytes":bytes.len(),
            "serialized_request_sha256":format!("{:x}", Sha256::digest(&bytes))})
        })
        .collect();
    json!({"description":"frozen public GitHub snapshots in fixture order; repeated material in concurrency mode, not unique issues",
        "serialization":"serde_json compact DecisionRequest JSON; not HTTP payload bytes or token estimates",
        "logical_calls_per_run":workload.len(),"serialized_workload_bytes":serialized.len(),
        "serialized_workload_sha256":format!("{:x}", Sha256::digest(&serialized)),"calls":calls})
}

fn save(path: &Path, report: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(report)?)
        .context("cannot write explicit report path")
}

fn percentiles(records: &[Value], field: &str) -> Value {
    let mut values: Vec<_> = records
        .iter()
        .filter_map(|record| record[field].as_u64())
        .collect();
    values.sort_unstable();
    let rank =
        |percentile: usize| values.get((percentile * values.len()).div_ceil(100).saturating_sub(1));
    json!({"p50":rank(50), "p95":rank(95)})
}

fn usage_sum(records: &[Value]) -> Value {
    let mut totals = serde_json::Map::new();
    for field in ["input_tokens", "output_tokens"] {
        let values: Vec<_> = records
            .iter()
            .filter_map(|record| record["usage"][field].as_u64())
            .collect();
        totals.insert(
            field.to_owned(),
            json!({"sum":values.iter().map(|value| u128::from(*value)).sum::<u128>(),
            "responses_reporting_field":values.len()}),
        );
    }
    json!({"scope":"returned usage only; missing responses and HTTP retry billing are not estimated","fields":totals})
}

async fn run(
    provider: &ConfiguredProvider,
    inputs: &[Input],
    concurrency: usize,
    provider_name: &str,
    model: &str,
    smoke: bool,
) -> Value {
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let started = Instant::now();
    let mut records: Vec<_> = stream::iter(inputs.iter().enumerate().map(|(index, input)| {
        let active = &active;
        let peak = &peak;
        async move {
            let queue_ms = started.elapsed().as_millis();
            let in_flight = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(in_flight, Ordering::SeqCst);
            let admitted = Instant::now();
            let response = provider.decide(&input.request).await;
            let duration_ms = admitted.elapsed().as_millis();
            active.fetch_sub(1, Ordering::SeqCst);
            let mut record = json!({"sample_index":index,"sample_id":input.sample["id"],
                "candidate":input.request.candidate_id,"input_id":input.request.input_id,
                "queue_ms":queue_ms,"duration_ms":duration_ms,"completed_ms":started.elapsed().as_millis(),
                "server_latency_ms":null,"success":false});
            match response {
                Err(error) => record["error_code"] = json!(error.code),
                Ok(response) => {
                    record["usage"] = json!(response.metadata.usage);
                    record["server_latency_ms"] = json!(response.metadata.server_latency_ms);
                    let native_valid = response.validate(&input.request).is_ok()
                        && response.metadata.provider == provider_name && response.metadata.model == model
                        && response.status == ResponseStatus::Complete && response.answers.len() == 7
                        && response.answers.iter().all(|answer| answer.status == AnswerStatus::Answered
                            && answer.probabilities.is_some())
                        && response.metadata.native_answers.as_ref().and_then(Value::as_object)
                            .is_some_and(|answers| answers.len() == 7);
                    let semantic = SemanticAnswers::from_response(&input.request, &response);
                    record["success"] = json!(native_valid && semantic.is_ok());
                    if record["success"] != true {
                        record["error_code"] = json!("native_acceptance_failed");
                    }
                    if smoke {
                        record["response"] = serde_json::to_value(&response).unwrap();
                        if let Ok(answers) = semantic {
                            let actual = serde_json::to_value(answers).unwrap();
                            let mismatches: Vec<_> = input.sample["expected"]["answers"].as_object().unwrap().iter()
                                .filter(|(key, expected)| actual[*key] != **expected)
                                .map(|(key, expected)| json!({"question":key,"expected":expected,"actual":actual[key]})).collect();
                            record["answers"] = actual;
                            record["label_mismatches"] = json!(mismatches);
                        }
                    }
                }
            }
            record
        }
    })).buffer_unordered(concurrency).collect().await;
    let wall_ms = started.elapsed().as_millis();
    records.sort_by_key(|record| record["sample_index"].as_u64().unwrap());
    let successful = records
        .iter()
        .filter(|record| record["success"] == true)
        .count();
    json!({"concurrency":concurrency,"request_count":records.len(),"successful_requests":successful,
        "failed_requests":records.len()-successful,"wall_ms":wall_ms,"peak_concurrency":peak.load(Ordering::SeqCst),
        "throughput_per_second":if wall_ms == 0 { None } else { Some(records.len() as f64 * 1000.0 / wall_ms as f64) },
        "duration_ms":percentiles(&records,"duration_ms"),"queue_ms":percentiles(&records,"queue_ms"),
        "usage_sum":usage_sum(&records),"samples":records})
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.report.is_absolute(),
        "--report must be an absolute path"
    );
    ensure!(
        args.mode == Mode::Workload || args.live,
        "real requests require explicit --live opt-in"
    );
    let inputs = workload(&args)?;
    let manifest = manifest(&inputs);
    if args.mode == Mode::Workload {
        return save(&args.report, &manifest);
    }
    let smoke = args.mode == Mode::Smoke;
    let order: &[usize] = if smoke { &[4] } else { &[4, 8, 8, 4] };
    let timeout_seconds = if smoke { 120 } else { 45 };
    let (kind, name, model) = args.provider.config();
    let fixture = fixtures::dataset();
    let mut report = json!({"purpose":"manual native transport acceptance/performance; label comparisons are diagnostic, not model quality measurements",
        "evidence_scope":"frozen public GitHub snapshots; no GitHub refresh",
        "fixture_labels_frozen_at":fixture["labelled_at"],"question_set_version":fixture["question_set_version"],
        "provider":name,"expected_model":model,"questions_per_request":7,"timeout_seconds":timeout_seconds,
        "concurrency_order":order,"maximum_logical_calls":inputs.len()*order.len(),"completed_logical_calls":0,
        "cache":"direct ConfiguredProvider::decide; local cache bypassed; remote caching uncontrolled",
        "http_attempts":"production bounded retries retained; logical calls do not establish HTTP attempt counts or retry billing",
        "provider_lifecycle":"fresh provider per run; construction/close excluded from wall time",
        "failure_policy":"finish current run then stop on any failure",
        "percentile_method":"nearest rank ceil(p*n), including failures; duration excludes queue",
        "workload":manifest,"state":"starting","runs":[]});
    save(&args.report, &report)?;
    for (index, &concurrency) in order.iter().enumerate() {
        let config = DecisionConfig {
            provider: kind,
            concurrency,
            timeout_seconds,
            candidate_budget: inputs.len(),
            ..DecisionConfig::default()
        };
        let provider = match ConfiguredProvider::new(&config) {
            Ok(provider) => provider,
            Err(error) => {
                report["state"] = json!("setup_failed");
                report["setup_error"] = json!({"run_index":index,"error_code":error.code});
                save(&args.report, &report)?;
                bail!("provider setup failed; see the explicit report path");
            }
        };
        let mut result = run(&provider, &inputs, concurrency, name, model, smoke).await;
        result["run_index"] = json!(index);
        result["provider_fingerprint"] = json!(provider.fingerprint());
        provider.close().await;
        let failed = result["failed_requests"] != 0;
        report["runs"].as_array_mut().unwrap().push(result);
        report["completed_logical_calls"] = json!((index + 1) * inputs.len());
        report["state"] = json!(if failed {
            "stopped_after_failure"
        } else {
            "running"
        });
        save(&args.report, &report)?;
        if failed {
            bail!("native requests failed; see the explicit report path");
        }
    }
    report["state"] = json!("completed");
    save(&args.report, &report)?;
    Ok(())
}
