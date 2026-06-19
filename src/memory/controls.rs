use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::memory::activation::{
    MemoryActivationEngine, MemoryActivationRequest, MemoryActivationResult,
};
use crate::memory::dreaming::{
    MemoryDreamEngine, MemoryDreamRequest, MemoryDreamResult, MemoryDreamSynthesizer,
};
use crate::memory::model::{MemoryHint, MemoryHintScopeType, MemoryHintStatus, MemoryHintType};
use crate::memory::store::MemoryStore;
use crate::memory::writeback::{MemoryWritebackGuard, MemoryWritebackReport};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MemoryRuntimeMode {
    #[default]
    Enabled,
    MemoryOff,
    NoWrite,
    Temporary,
}

impl MemoryRuntimeMode {
    pub fn allows_decision_hints(self) -> bool {
        self != Self::MemoryOff
    }

    pub fn allows_activation_persistence(self) -> bool {
        self == Self::Enabled
    }

    pub fn allows_writeback(self) -> bool {
        self == Self::Enabled
    }

    pub fn allows_dreaming(self) -> bool {
        self == Self::Enabled
    }

    pub fn allows_ingest(self) -> bool {
        matches!(self, Self::Enabled | Self::NoWrite)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHintScope {
    pub scope_type: MemoryHintScopeType,
    pub scope_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryDecisionHintRequest {
    pub mode: MemoryRuntimeMode,
    pub hint_type: Option<MemoryHintType>,
    pub scope: Option<MemoryHintScope>,
    pub now: Option<String>,
    pub limit: usize,
}

impl Default for MemoryDecisionHintRequest {
    fn default() -> Self {
        Self {
            mode: MemoryRuntimeMode::Enabled,
            hint_type: None,
            scope: None,
            now: None,
            limit: usize::MAX,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemoryDecisionHint {
    pub hint: MemoryHint,
    pub effective_weight: f64,
}

pub struct MemoryControlPlane;

impl MemoryControlPlane {
    pub fn decision_eligible_hints(
        store: &MemoryStore,
        request: &MemoryDecisionHintRequest,
    ) -> Result<Vec<MemoryDecisionHint>> {
        if !request.mode.allows_decision_hints() || request.limit == 0 {
            return Ok(Vec::new());
        }

        let hints = store.list_hints()?;
        if scope_is_suppressed(&hints, request.scope.as_ref()) {
            return Ok(Vec::new());
        }

        let mut eligible = hints
            .into_iter()
            .filter(|hint| hint_matches_request(hint, request))
            .filter(|hint| !hint_is_expired(hint, request.now.as_deref()))
            .filter_map(|hint| {
                let effective_weight = effective_weight(&hint)?;
                Some(MemoryDecisionHint {
                    hint,
                    effective_weight,
                })
            })
            .collect::<Vec<_>>();

        eligible.sort_by(|left, right| {
            hint_priority(&right.hint)
                .cmp(&hint_priority(&left.hint))
                .then_with(|| {
                    right
                        .effective_weight
                        .partial_cmp(&left.effective_weight)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| left.hint.id.cmp(&right.hint.id))
        });
        eligible.truncate(request.limit);
        Ok(eligible)
    }

    pub fn activate(
        store: &MemoryStore,
        request: &MemoryActivationRequest,
        mode: MemoryRuntimeMode,
    ) -> Result<MemoryActivationResult> {
        if mode == MemoryRuntimeMode::MemoryOff {
            return Ok(MemoryActivationResult {
                run_id: request.run_id.clone(),
                items: Vec::new(),
            });
        }

        let mut controlled_request = request.clone();
        if !mode.allows_activation_persistence() {
            controlled_request.persist_trace = false;
        }
        MemoryActivationEngine::activate(store, &controlled_request)
    }

    pub fn apply_writeback(
        store: &MemoryStore,
        activation_run_id: &str,
        occurred_at: &str,
        mode: MemoryRuntimeMode,
    ) -> Result<MemoryWritebackReport> {
        if !mode.allows_writeback() {
            return Ok(MemoryWritebackReport {
                activation_run_id: activation_run_id.to_string(),
                recalled: 0,
                resource_decremented: 0,
                reinforced: 0,
                edge_reinforced: 0,
                skipped: 0,
            });
        }
        MemoryWritebackGuard::apply(store, activation_run_id, occurred_at)
    }

    pub fn dream(
        store: &MemoryStore,
        request: &MemoryDreamRequest,
        synthesizer: Option<&dyn MemoryDreamSynthesizer>,
        mode: MemoryRuntimeMode,
    ) -> Result<Option<MemoryDreamResult>> {
        if !mode.allows_dreaming() {
            return Ok(None);
        }
        MemoryDreamEngine::dream(store, request, synthesizer).map(Some)
    }
}

fn hint_matches_request(hint: &MemoryHint, request: &MemoryDecisionHintRequest) -> bool {
    if request
        .hint_type
        .is_some_and(|hint_type| hint.hint_type != hint_type)
    {
        return false;
    }
    scope_matches(&hint.scope_type, &hint.scope_ref, request.scope.as_ref())
}

fn scope_matches(
    hint_scope_type: &MemoryHintScopeType,
    hint_scope_ref: &str,
    request_scope: Option<&MemoryHintScope>,
) -> bool {
    let Some(request_scope) = request_scope else {
        return true;
    };
    (*hint_scope_type == MemoryHintScopeType::Global)
        || (*hint_scope_type == request_scope.scope_type
            && hint_scope_ref == request_scope.scope_ref)
}

fn scope_is_suppressed(hints: &[MemoryHint], request_scope: Option<&MemoryHintScope>) -> bool {
    hints
        .iter()
        .filter(|hint| hint.status == MemoryHintStatus::Suppressed)
        .any(|hint| scope_matches(&hint.scope_type, &hint.scope_ref, request_scope))
}

fn effective_weight(hint: &MemoryHint) -> Option<f64> {
    match hint.status {
        MemoryHintStatus::Approved | MemoryHintStatus::Pinned => Some(hint.weight),
        MemoryHintStatus::Deprioritized => Some(hint.weight * 0.5),
        MemoryHintStatus::Candidate
        | MemoryHintStatus::Rejected
        | MemoryHintStatus::Suppressed
        | MemoryHintStatus::Stale
        | MemoryHintStatus::Tombstoned => None,
    }
}

fn hint_priority(hint: &MemoryHint) -> u8 {
    match hint.status {
        MemoryHintStatus::Pinned => 3,
        MemoryHintStatus::Approved => 2,
        MemoryHintStatus::Deprioritized => 1,
        MemoryHintStatus::Candidate
        | MemoryHintStatus::Rejected
        | MemoryHintStatus::Suppressed
        | MemoryHintStatus::Stale
        | MemoryHintStatus::Tombstoned => 0,
    }
}

fn hint_is_expired(hint: &MemoryHint, now: Option<&str>) -> bool {
    let (Some(expires_at), Some(now)) = (hint.expires_at.as_deref(), now) else {
        return false;
    };
    let Ok(expires_at) = DateTime::parse_from_rfc3339(expires_at) else {
        return false;
    };
    let Ok(now) = DateTime::parse_from_rfc3339(now) else {
        return false;
    };
    expires_at.with_timezone(&Utc) <= now.with_timezone(&Utc)
}
