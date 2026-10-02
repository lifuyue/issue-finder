//! Aliyun's native TypeSafe System One protocol, not a chat completion endpoint.
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::contract::*;
use super::http::{validate_endpoint, DecisionHttp};
use super::native::{parse_response, request_body};

pub const MODEL: &str = "decision-model-preview";
const ENDPOINT_PATH: &str = "/compatible-mode/v1/systemone";

pub struct AliyunProvider {
    endpoint: String,
    api_key: String,
    timeout: Duration,
    http: DecisionHttp,
}

impl AliyunProvider {
    pub fn new(
        endpoint: String,
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, ProviderError> {
        validate_endpoint(&endpoint)?;
        let url = url::Url::parse(&endpoint).map_err(|_| configuration("invalid endpoint"))?;
        if url.path() != ENDPOINT_PATH || url.query().is_some() || url.fragment().is_some() {
            return Err(configuration(
                "Aliyun endpoint must include /compatible-mode/v1/systemone without a query or fragment",
            ));
        }
        if api_key.trim().is_empty() {
            return Err(configuration("Aliyun API key is required"));
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
        let body = request_body(request, MODEL)?;
        let start = Instant::now();
        let raw = self.http.post(&self.endpoint, &self.api_key, &body).await?;
        parse_response(
            request,
            raw,
            "aliyun_decision",
            MODEL,
            start.elapsed().as_millis().min(u64::MAX as u128) as u64,
        )
    }
}

impl Provider for AliyunProvider {
    fn fingerprint(&self) -> String {
        let endpoint_hash = format!("{:x}", Sha256::digest(self.endpoint.as_bytes()));
        format!(
            "aliyun:{MODEL}:systemone-v1:{endpoint_hash}:timeout-ms={}",
            self.timeout.as_millis()
        )
    }

    fn decide<'a>(&'a self, request: &'a DecisionRequest) -> DecisionFuture<'a> {
        Box::pin(self.decide_inner(request))
    }
}

fn configuration(message: &str) -> ProviderError {
    ProviderError::new("invalid_configuration", message)
}
