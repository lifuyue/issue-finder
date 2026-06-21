use serde_json::json;

use super::model::{DispatchFailureClass, NewDispatchFailure};

pub fn execution_failure(run_id: &str, phase: &str, error: &anyhow::Error) -> NewDispatchFailure {
    let message = error.to_string();
    let (failure_class, code, retryable) = classify_error(&message);
    NewDispatchFailure {
        run_id: run_id.to_string(),
        phase: phase.to_string(),
        failure_class,
        code: code.to_string(),
        retryable,
        message,
        details_json: json!({}),
    }
}

fn classify_error(message: &str) -> (DispatchFailureClass, &'static str, bool) {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("capability") || normalized.contains("does not support") {
        (
            DispatchFailureClass::Capability,
            "capability_unavailable",
            false,
        )
    } else if normalized.contains("approval") || normalized.contains("not approved") {
        (DispatchFailureClass::Policy, "approval_required", false)
    } else if normalized.contains("codex app-server") || normalized.contains("adapter") {
        (DispatchFailureClass::Adapter, "adapter_error", true)
    } else if normalized.contains("github") || normalized.contains("a2a") {
        (DispatchFailureClass::External, "external_error", true)
    } else if normalized.contains("sqlite") || normalized.contains("database") {
        (DispatchFailureClass::Storage, "storage_error", true)
    } else {
        (DispatchFailureClass::Unknown, "unknown_error", false)
    }
}
