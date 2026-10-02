//! Shared transport for native decision APIs. Never include credentials, state,
//! raw response bodies or request URLs in diagnostic errors.
use std::time::Duration;

use reqwest::{header, redirect, Client, StatusCode};
use serde_json::Value;
use tokio::time::Instant;
use url::Url;

use super::contract::ProviderError;

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

fn invalid(message: &str) -> ProviderError {
    ProviderError::new("invalid_configuration", message)
}

fn loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

pub(super) fn validate_endpoint(endpoint: &str) -> Result<(), ProviderError> {
    let url = Url::parse(endpoint).map_err(|_| invalid("Decision endpoint must be a valid URL"))?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback(&url)))
    {
        return Err(invalid("Decision endpoint requires HTTPS without credentials, query or fragment; HTTP is allowed only for loopback tests"));
    }
    Ok(())
}

pub(super) struct DecisionHttp {
    client: Client,
    local_client: Client,
    timeout: Duration,
}

impl DecisionHttp {
    pub(super) fn new(timeout: Duration) -> Result<Self, ProviderError> {
        if timeout.is_zero() {
            return Err(invalid("Decision timeout must be positive"));
        }
        let client = Client::builder()
            .redirect(redirect::Policy::none())
            .connect_timeout(timeout.min(Duration::from_secs(10)))
            .build()
            .map_err(|_| invalid("Cannot initialize decision HTTP client"))?;
        let local_client = Client::builder()
            .no_proxy()
            .redirect(redirect::Policy::none())
            .build()
            .map_err(|_| invalid("Cannot initialize loopback decision client"))?;
        Ok(Self {
            client,
            local_client,
            timeout,
        })
    }

    pub(super) async fn post(
        &self,
        endpoint: &str,
        api_key: &str,
        body: &Value,
    ) -> Result<Value, ProviderError> {
        validate_endpoint(endpoint)?;
        let mut authorization = header::HeaderValue::from_str(&format!("Bearer {api_key}"))
            .map_err(|_| invalid("Decision API credential is not a valid header value"))?;
        if api_key.trim().is_empty() {
            return Err(ProviderError::new(
                "authentication_setup",
                "Decision API credential is empty",
            ));
        }
        authorization.set_sensitive(true);
        let url = Url::parse(endpoint).map_err(|_| invalid("Invalid decision endpoint"))?;
        // Loopback mocks must stay on the machine. Remote calls retain inherited
        // proxies and platform CA trust, including network-secret substitution.
        let client = if loopback(&url) {
            &self.local_client
        } else {
            &self.client
        };
        let deadline = Instant::now() + self.timeout;
        tokio::time::timeout_at(deadline, async {
            for attempt in 0..=1 {
                let mut response = client
                    .post(url.clone())
                    .header(header::AUTHORIZATION, authorization.clone())
                    .json(body)
                    .send()
                    .await
                    .map_err(|_| ProviderError::new("transport_error", "Decision request transport failed; acceptance is unknown, so it was not resubmitted"))?;
                let status = response.status();
                let retry_after = response.headers().get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok()).and_then(retry_delay);
                if response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
                    return Err(ProviderError::new("invalid_output", "Decision response exceeds the size limit"));
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::new("transport_error", "Decision response was lost; request was not resubmitted"))? {
                    if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                        return Err(ProviderError::new("invalid_output", "Decision response exceeds the size limit"));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                if status.is_success() {
                    return serde_json::from_slice(&bytes).map_err(|_| ProviderError::new("invalid_output", "Decision service did not return valid JSON"));
                }
                let error = status_error(status, &bytes);
                if attempt == 0 && matches!(error.code.as_str(), "rate_limited" | "overloaded") {
                    let delay = retry_after.unwrap_or(Duration::from_millis(150))
                        .saturating_add(jitter());
                    if delay >= deadline.saturating_duration_since(Instant::now()) {
                        return Err(error);
                    }
                    tokio::time::sleep(delay).await;
                    continue;
                }
                return Err(error);
            }
            unreachable!()
        }).await.map_err(|_| ProviderError::new("timeout", "Decision call exceeded its timeout after admission; no uncertain request was resubmitted"))?
    }
}

fn status_error(status: StatusCode, body: &[u8]) -> ProviderError {
    // Inspect only to classify. The service may echo input or auth in its body.
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let quota = [
        "insufficient_quota",
        "quota_exceeded",
        "quotaexhausted",
        "quota exhausted",
        "quota exceeded",
        "daily quota",
        "daily limit",
        "exceeded your free",
        "insufficient balance",
        "insufficient credit",
        "freeallocationexhausted",
        "arrearage",
    ]
    .iter()
    .any(|marker| text.contains(marker));
    let code = if status == StatusCode::UNAUTHORIZED {
        "authentication_failed"
    } else if status == StatusCode::PAYMENT_REQUIRED || quota {
        "quota_exhausted"
    } else if status == StatusCode::FORBIDDEN {
        "permission_denied"
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        "rate_limited"
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        "overloaded"
    } else if status.is_redirection() {
        "redirect_rejected"
    } else if status.is_client_error() {
        "invalid_request"
    } else {
        "provider_failed"
    };
    ProviderError::new(
        code,
        format!(
            "Decision service returned HTTP {}; response body omitted",
            status.as_u16()
        ),
    )
}

fn retry_delay(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}

fn jitter() -> Duration {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    Duration::from_millis(10 + (time ^ SEQUENCE.fetch_add(1, Ordering::Relaxed)) % 41)
}
