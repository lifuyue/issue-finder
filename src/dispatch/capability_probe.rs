use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::Value;

use super::model::{
    AdapterProbeResult, AdapterProbeStatus, AgentCapability, CapabilityStatus,
    NewAdapterProbeResult,
};
use super::store::DispatchStore;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProbeReport {
    pub agent_id: String,
    pub refreshed: bool,
    pub probes: Vec<AdapterProbeResult>,
}

pub fn probe_agent(
    store: &DispatchStore,
    agent_id: &str,
    refresh: bool,
) -> Result<AgentProbeReport> {
    let agent = store.get_agent_profile(agent_id)?;
    let capabilities = store.list_agent_capabilities(agent_id)?;
    let mut probes = Vec::new();
    for capability in capabilities {
        if !refresh {
            if let Some(cached) = fresh_successful_probe(store, &capability)? {
                probes.push(cached);
                continue;
            }
        }
        probes.push(record_probe(store, &agent.adapter, capability)?);
    }
    Ok(AgentProbeReport {
        agent_id: agent.id,
        refreshed: refresh,
        probes,
    })
}

fn fresh_successful_probe(
    store: &DispatchStore,
    capability: &AgentCapability,
) -> Result<Option<AdapterProbeResult>> {
    let Some(probe) = store.latest_adapter_probe(&capability.agent_id, capability.capability)?
    else {
        return Ok(None);
    };
    if probe.status != AdapterProbeStatus::Supported {
        return Ok(None);
    }
    let Some(expires_at) = probe.expires_at.as_deref() else {
        return Ok(None);
    };
    let expires_at = DateTime::parse_from_rfc3339(expires_at)
        .map(|value| value.with_timezone(&Utc))
        .ok();
    Ok(expires_at
        .filter(|value| *value > Utc::now())
        .map(|_| probe))
}

fn record_probe(
    store: &DispatchStore,
    adapter: &str,
    capability: AgentCapability,
) -> Result<AdapterProbeResult> {
    let startup_probe_status = capability
        .details_json
        .pointer("/startup/probe/status")
        .and_then(Value::as_str);
    let binary_unavailable = startup_probe_status == Some("binary_unavailable");
    let status = if binary_unavailable {
        AdapterProbeStatus::Failed
    } else if capability.status == CapabilityStatus::Unsupported {
        AdapterProbeStatus::Unsupported
    } else {
        AdapterProbeStatus::Supported
    };
    let expires_at = (status == AdapterProbeStatus::Supported)
        .then(|| (Utc::now() + Duration::hours(24)).to_rfc3339());
    let method = capability
        .details_json
        .get("method")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let error_code = if binary_unavailable {
        Some("binary_unavailable".to_string())
    } else if capability.status == CapabilityStatus::Unsupported {
        Some("capability_unsupported".to_string())
    } else {
        None
    };
    store.record_adapter_probe(NewAdapterProbeResult {
        agent_id: capability.agent_id,
        adapter: adapter.to_string(),
        capability: capability.capability,
        method,
        status,
        protocol_version: capability
            .details_json
            .get("protocol")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        expires_at,
        error_code,
        details_json: capability.details_json,
    })
}
