//! Cloudflare's native Clef Flash decision API and its REST response envelope.
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::contract::*;
use super::http::{validate_endpoint, DecisionHttp};
use super::native::{parse_response, request_body};

pub const MODEL: &str = "clef-flash";
const MODEL_PATH: &str = "/ai/run/@cf/cloudflare/clef-flash";

pub struct ClefProvider {
    endpoint: String,
    api_key: String,
    timeout: Duration,
    http: DecisionHttp,
}

impl ClefProvider {
    pub fn new(
        endpoint: String,
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, ProviderError> {
        validate_endpoint(&endpoint)?;
        let url = url::Url::parse(&endpoint)
            .map_err(|_| configuration("Invalid Cloudflare Clef endpoint"))?;
        let account = url
            .path()
            .strip_prefix("/client/v4/accounts/")
            .and_then(|path| path.strip_suffix(MODEL_PATH));
        if !account.is_some_and(|account| {
            !account.is_empty()
                && account
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        }) {
            return Err(configuration(
                "Cloudflare endpoint must use /client/v4/accounts/{ACCOUNT_ID}/ai/run/@cf/cloudflare/clef-flash",
            ));
        }
        if api_key.trim().is_empty() {
            return Err(configuration("Cloudflare Workers AI API token is required"));
        }
        Ok(Self {
            endpoint,
            api_key,
            timeout,
            http: DecisionHttp::new(timeout)?,
        })
    }

    async fn decide_inner(
        &self,
        request: &DecisionRequest,
    ) -> Result<DecisionResponse, ProviderError> {
        validate_request(request)?;
        let body = request_body(request, MODEL)?;
        let start = Instant::now();
        let raw = self.http.post(&self.endpoint, &self.api_key, &body).await?;
        let result = unwrap_envelope(raw)?;
        validate_usage(&result)?;
        let response = parse_response(
            request,
            result,
            "cloudflare_clef_flash",
            MODEL,
            start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        )?;
        validate_choices(&response)?;
        Ok(response)
    }
}

impl Provider for ClefProvider {
    fn fingerprint(&self) -> String {
        let endpoint_hash = format!("{:x}", Sha256::digest(self.endpoint.as_bytes()));
        format!(
            "cloudflare:{MODEL}:systemone-v1:{endpoint_hash}:timeout-ms={}",
            self.timeout.as_millis()
        )
    }

    fn decide<'a>(&'a self, request: &'a DecisionRequest) -> DecisionFuture<'a> {
        Box::pin(self.decide_inner(request))
    }
}

fn validate_request(request: &DecisionRequest) -> Result<(), ProviderError> {
    request.validate()?;
    if !(1..=64).contains(&request.questions.len()) {
        return Err(contract("Clef supports 1 through 64 questions per request"));
    }
    for question in &request.questions {
        if question.id.len() > 100
            || !question
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        {
            return Err(contract(
                "Clef question IDs require 1 through 100 ASCII letters, digits, underscores, dots or hyphens",
            ));
        }
        match &question.kind {
            QuestionKind::Choice { options } if !(2..=255).contains(&options.len()) => {
                return Err(contract("Clef choice requires 2 through 255 options"));
            }
            QuestionKind::Score { levels } if !(2..=10).contains(&levels.len()) => {
                return Err(contract("Clef score requires 2 through 10 ordered levels"));
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Envelope {
    success: bool,
    errors: Vec<Value>,
    result: Option<Value>,
}

fn unwrap_envelope(raw: Value) -> Result<Value, ProviderError> {
    let envelope: Envelope = serde_json::from_value(raw)
        .map_err(|_| protocol("Cloudflare response is missing its required REST envelope"))?;
    if !envelope.success || !envelope.errors.is_empty() {
        return Err(envelope_error(&envelope.errors));
    }
    envelope
        .result
        .ok_or_else(|| protocol("Cloudflare success envelope is missing its native result"))
}

fn envelope_error(errors: &[Value]) -> ProviderError {
    // Cloudflare errors can echo submitted state or credentials. Inspect only
    // to classify, then emit a fixed diagnostic rather than the service text.
    let text = serde_json::to_string(errors)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let code = if [
        "quota",
        "free allocation",
        "freeallocationexhausted",
        "billing",
        "payment required",
    ]
    .iter()
    .any(|marker| text.contains(marker))
    {
        "quota_exhausted"
    } else if ["permission", "forbidden", "not authorized", "access denied"]
        .iter()
        .any(|marker| text.contains(marker))
    {
        "permission_denied"
    } else if [
        "authentication",
        "unauthorized",
        "invalid token",
        "expired token",
    ]
    .iter()
    .any(|marker| text.contains(marker))
        || errors
            .iter()
            .any(|error| error["code"].as_u64() == Some(10000))
    {
        "authentication_failed"
    } else if text.contains("rate limit") || text.contains("too many requests") {
        "rate_limited"
    } else if text.contains("overloaded") || text.contains("temporarily unavailable") {
        "overloaded"
    } else if text.contains("invalid request") || text.contains("invalid input") {
        "invalid_request"
    } else {
        "provider_failed"
    };
    ProviderError::new(
        code,
        "Cloudflare returned a failed decision envelope; service error details omitted",
    )
}

fn validate_usage(result: &Value) -> Result<(), ProviderError> {
    let usage = result.get("usage").and_then(Value::as_object);
    if !usage.is_some_and(|usage| {
        usage.get("input_tokens").and_then(Value::as_u64).is_some()
            && usage.get("output_tokens").and_then(Value::as_u64) == Some(0)
    }) {
        return Err(protocol(
            "Clef requires integer input_tokens and zero output_tokens for native decisions",
        ));
    }
    Ok(())
}

fn validate_choices(response: &DecisionResponse) -> Result<(), ProviderError> {
    for answer in &response.answers {
        if let (Some(Answer::Choice(chosen)), Some(ProbabilityInfo::Choice { distribution })) =
            (&answer.answer, &answer.probabilities)
        {
            let selected = distribution
                .iter()
                .find(|entry| &entry.answer == chosen)
                .ok_or_else(|| protocol("Clef choice is missing its native probability"))?;
            if distribution
                .iter()
                .any(|entry| entry.probability > selected.probability)
            {
                return Err(protocol(
                    "Clef choice must select a highest-probability option",
                ));
            }
        }
    }
    Ok(())
}

fn configuration(message: &str) -> ProviderError {
    ProviderError::new("invalid_configuration", message)
}

fn contract(message: &str) -> ProviderError {
    ProviderError::new("invalid_contract", message)
}

fn protocol(message: &str) -> ProviderError {
    ProviderError::new("invalid_provider_response", message)
}
